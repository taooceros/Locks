use derivative::Derivative;
use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
use std::fmt::Debug;
use std::sync::Mutex;
use std::thread::current;
use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    mem::transmute,
    ptr,
    sync::atomic::{AtomicPtr, Ordering::*},
};

use crossbeam::utils::{Backoff, CachePadded};

use thread_local::ThreadLocal;

use crate::rand;
use crate::{
    atomic_extension::AtomicExtension,
    dlock2::{DLock2, DLock2Delegate},
    sequential_priority_queue::SequentialPriorityQueue,
    spin_lock::RawSpinLock,
    RawSimpleLock,
};

use arrayvec::ArrayVec;

mod buffer;

use self::buffer::ConcurrentRingBuffer;

use super::node::Node;

const CLEAN_UP_AGE: u32 = 500;

#[derive(Derivative, Debug)]
#[derivative(PartialEq, Eq, PartialOrd, Ord)]
pub struct UsageNode<'a, I> {
    usage: u64,
    tie_breaker: u64,
    #[derivative(PartialEq = "ignore", PartialOrd = "ignore", Ord = "ignore")]
    node: &'a Node<I>,
}

impl<T> Clone for UsageNode<'_, T> {
    fn clone(&self) -> Self {
        UsageNode {
            usage: self.usage,
            tie_breaker: self.tie_breaker,
            node: self.node,
        }
    }
}

impl<T> Copy for UsageNode<'_, T> {}

unsafe impl<'a, I: Send> Sync for UsageNode<'a, I> {}

#[derive(Debug)]
pub struct FCPQ<T, I, PQ, F, L>
where
    T: Send + Sync,
    I: Send + 'static,
    PQ: SequentialPriorityQueue<UsageNode<'static, I>> + Debug,
    F: Fn(&mut T, I) -> I,
    L: RawSimpleLock,
{
    combiner_lock: CachePadded<L>,
    delegate: F,
    job_queue: SyncUnsafeCell<PQ>,
    waiting_nodes: ConcurrentRingBuffer<(AtomicPtr<Node<I>>, u64), 64>,
    data: SyncUnsafeCell<T>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<I>>>,
}

impl<T, I, PQ, F, L> FCPQ<T, I, PQ, F, L>
where
    T: Send + Sync,
    I: Send,
    PQ: SequentialPriorityQueue<UsageNode<'static, I>> + Debug,
    F: DLock2Delegate<T, I>,
    L: RawSimpleLock,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            combiner_lock: CachePadded::new(L::new()),
            delegate,
            job_queue: PQ::new().into(),
            waiting_nodes: ConcurrentRingBuffer::new(),
            data: SyncUnsafeCell::new(data),
            local_node: ThreadLocal::new(),
        }
    }

    fn push_node(&self, node: &Node<I>) {
        self.waiting_nodes.push((
            AtomicPtr::new(node as *const _ as *mut Node<I>),
            current().id().as_u64().into(),
        ));
        node.active.store_release(true);
    }

    fn push_if_unactive(&self, node: &mut Node<I>) {
        if node.active.load(Acquire) {
            return;
        }

        self.push_node(node);
    }

    fn combine(&self) {
        #[cfg(feature = "combiner_stat")]
        let mut aux: u32 = 0;
        let mut begin: u64;

        unsafe {
            begin = __rdtscp(&mut aux);
        }

        const H: usize = 32;

        // only one thread would combine so this is safe
        let job_queue = unsafe { &mut *self.job_queue.get() };

        if !self.waiting_nodes.empty() {
            for (node, id) in self.waiting_nodes.iter() {
                unsafe {
                    let node = &*node.load_acquire();
                    node.active.store_release(true);
                    job_queue.push(UsageNode {
                        usage: node.usage,
                        tie_breaker: id,
                        node,
                    });
                }
            }
        }

        let mut buffer = ConstGenericRingBuffer::<_, 16>::new();

        unsafe {
            for _ in 0..H {
                let current = job_queue.pop();

                if current.is_none() {
                    break;
                }

                let mut current = current.unwrap_unchecked();

                let node = current.node;

                if !node.complete.load(Acquire) {
                    node.data.get().write(
                        (self.delegate)(
                            self.data.get().as_mut().unwrap_unchecked(),
                            ptr::read(node.data.get()),
                        )
                        .into(),
                    );

                    let end = __rdtscp(&mut aux);

                    current.usage += end - begin;

                    begin = end;

                    node.complete.store(true, Release);

                    job_queue.push(current);
                } else {
                    // if the buffer is full then push the nodes back to the job queue
                    if buffer.is_full() {
                        for node in buffer.drain() {
                            job_queue.push(node);
                        }
                    }

                    // if the node is not ready to execute then push it back to the buffer
                    buffer.push(current);
                }
            }

            if !buffer.is_empty() {
                for node in buffer.drain() {
                    job_queue.push(node);
                }
            }
        }

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            (*self.local_node.get().unwrap().get()).combiner_time_stat += end - begin;
        }
    }
}

unsafe impl<T, PQ, I, F> DLock2<I> for FCPQ<T, I, PQ, F, RawSpinLock>
where
    T: Send + Sync,
    PQ: SequentialPriorityQueue<UsageNode<'static, I>> + Debug + Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn lock(&self, data: I) -> I {
        let node = self.local_node.get_or(|| SyncUnsafeCell::new(Node::new()));

        let node = unsafe { &mut *node.get() };

        node.data = data.into();
        node.complete.store(false, Release);

        'outer: loop {
            self.push_if_unactive(node);

            if self.combiner_lock.try_lock() {
                self.combine();
                self.combiner_lock.unlock();

                if node.complete.load(Acquire) {
                    break 'outer;
                }
            } else {
                let backoff = Backoff::new();
                loop {
                    if node.complete.load(Acquire) {
                        break 'outer;
                    }
                    backoff.snooze();
                    if backoff.is_completed() {
                        continue 'outer;
                    }
                }
            }
        }

        unsafe { ptr::read(node.data.get()) }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        unsafe {
            self.local_node
                .get()
                .unwrap()
                .get()
                .read()
                .combiner_time_stat
                .into()
        }
    }
}
