use derivative::Derivative;
use lock_api::RawMutex;
use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
use std::fmt::Debug;
use std::thread::current;
use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    ptr,
    sync::atomic::{AtomicPtr, Ordering::*},
};

use crate::dlock2::CombinerSample;
use crossbeam::utils::{Backoff, CachePadded};

use thread_local::ThreadLocal;

use crate::{
    atomic_extension::AtomicExtension,
    dlock2::{DLock2, DLock2Delegate},
    sequential_priority_queue::SequentialPriorityQueue,
};


mod buffer;

use self::buffer::ConcurrentRingBuffer;

use super::node::Node;

// const CLEAN_UP_AGE: u32 = 500;

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
    L: RawMutex,
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
    L: RawMutex,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            combiner_lock: CachePadded::new(L::INIT),
            delegate,
            job_queue: PQ::new().into(),
            waiting_nodes: ConcurrentRingBuffer::new(),
            data: SyncUnsafeCell::new(data),
            local_node: ThreadLocal::new(),
        }
    }

    fn push_node(&self, node: &Node<I>) {
        node.active.store(true, Release);
        self.waiting_nodes.push((
            AtomicPtr::new(node as *const _ as *mut Node<I>),
            current().id().as_u64().into(),
        ));
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

        #[cfg(feature = "combiner_stat")]
        let mut combine_size = 0;

        let mut begin: u64;

        unsafe {
            begin = __rdtscp(&mut aux);
        }

        const H: usize = 64;

        // only one thread would combine so this is safe
        let job_queue: &mut PQ = unsafe { &mut *self.job_queue.get() };

        if !self.waiting_nodes.empty() {
            let iterator = unsafe { self.waiting_nodes.iter() };

            let size = iterator.size_hint();

            let mut count = 0;

            for (node, id) in iterator {
                count += 1;
                unsafe {
                    let node = &*node.load_acquire();
                    job_queue.push(UsageNode {
                        usage: node.usage.load_acquire(),
                        tie_breaker: id,
                        node,
                    });
                }
            }

            assert!(count == size.0);
        }

        let mut buffer = ConstGenericRingBuffer::<UsageNode<I>, 4>::new();

        unsafe {
            for _ in 0..H {
                let current = job_queue.pop();

                if current.is_none() {
                    break;
                }

                let mut current = current.unwrap_unchecked();

                let node = current.node;

                if !node.complete.load(Acquire) {
                    // alternatively we can potentially save one __rdtscp by using `end` here
                    // which would result in a slightly inaccurate usage
                    begin = __rdtscp(&mut aux);

                    node.data.get().write(
                        (self.delegate)(
                            self.data.get().as_mut().unwrap_unchecked(),
                            ptr::read(node.data.get()),
                        )
                        .into(),
                    );

                    let end = __rdtscp(&mut aux);

                    current.usage += end - begin;

                    node.complete.store(true, Release);

                    combine_size += 1;

                    job_queue.push(current);
                } else {
                    // if the buffer is full then push the nodes back to the job queue
                    if buffer.is_full() {
                        for node in buffer.drain() {
                            if node.node.complete.load(Acquire) {
                                node.node.usage.store_release(node.usage);
                                node.node.active.store_release(false);
                            } else {
                                job_queue.push(node);
                            }
                        }
                    }

                    // if the node is not ready to execute then push it back to the buffer
                    buffer.push(current);
                }
            }

            for node in buffer.drain() {
                if node.node.complete.load(Acquire) {
                    node.node.usage.store_release(node.usage);
                    node.node.active.store_release(false);
                } else {
                    job_queue.push(node);
                }
            }
        }

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            let combiner_stat = &mut (*self.local_node.get().unwrap().get()).combiner_stat;

            combiner_stat.combine_time.push(end - begin);
            *combiner_stat.combine_size.entry(combine_size).or_default() += 1;
        }
    }
}

unsafe impl<T, PQ, I, F, L> DLock2<I> for FCPQ<T, I, PQ, F, L>
where
    T: Send + Sync,
    PQ: SequentialPriorityQueue<UsageNode<'static, I>> + Debug + Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
    L: RawMutex + Send + Sync,
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

                unsafe {
                    self.combiner_lock.unlock();
                }

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
    fn get_combine_stat(&self) -> Option<&CombinerSample> {
        unsafe { self.local_node.get().map(|x| &(*x.get()).combiner_stat) }
    }
}
