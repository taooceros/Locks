use derivative::Derivative;
use lock_api::RawMutex;
use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::thread::current;
use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    ptr,
    sync::atomic::{AtomicPtr, Ordering::*},
};

use crossbeam::utils::{Backoff, CachePadded};

use thread_local::ThreadLocal;

use crate::{
    atomic_extension::AtomicExtension,
    dlock2::{DLock2, DLock2Delegate},
    sequential_priority_queue::SequentialPriorityQueue,
    spin_lock::RawSpinLock,
};

mod buffer;

use self::buffer::ConcurrentRingBuffer;

use super::node::Node;

const CLEAN_UP_AGE: u32 = 500;

/// Maximum number of combining passes a node may wait before its usage is
/// clamped to the current queue minimum.  Prevents unbounded starvation under
/// adversarial arrival patterns where one long-CS thread accumulates high usage
/// and is perpetually deprioritized by a stream of short-CS newcomers.
const STARVATION_THRESHOLD: u64 = 8;

#[derive(Derivative, Debug)]
#[derivative(PartialEq, Eq, PartialOrd, Ord)]
pub struct UsageNode<'a, I> {
    usage: u64,
    tie_breaker: u64,
    #[derivative(PartialEq = "ignore", PartialOrd = "ignore", Ord = "ignore")]
    node: &'a Node<I>,
    /// Combining pass number when this node was (re-)inserted into the PQ.
    /// Used to detect starvation: if `current_pass - pass_entered > K`, clamp
    /// usage to the queue minimum.
    #[derivative(PartialEq = "ignore", PartialOrd = "ignore", Ord = "ignore")]
    pass_entered: u64,
}

impl<T> Clone for UsageNode<'_, T> {
    fn clone(&self) -> Self {
        UsageNode {
            usage: self.usage,
            tie_breaker: self.tie_breaker,
            node: self.node,
            pass_entered: self.pass_entered,
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
    /// Running total of CS time across all served requests (combiner-only access)
    total_usage: SyncUnsafeCell<u64>,
    /// Running count of served requests (combiner-only access)
    total_served: SyncUnsafeCell<u64>,
    /// Monotonically increasing combining pass counter (combiner-only access)
    combine_pass: SyncUnsafeCell<u64>,
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
            total_usage: SyncUnsafeCell::new(0),
            total_served: SyncUnsafeCell::new(0),
            combine_pass: SyncUnsafeCell::new(0),
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
        let mut aux: u32 = 0;
        let mut begin: u64;

        unsafe {
            begin = __rdtscp(&mut aux);
        }

        const H: usize = 64;

        // only one thread would combine so this is safe
        let job_queue: &mut PQ = unsafe { &mut *self.job_queue.get() };

        // Advance the combining pass counter (combiner-only, no atomics needed)
        let current_pass = unsafe {
            let pass = &mut *self.combine_pass.get();
            *pass += 1;
            *pass
        };

        if !self.waiting_nodes.empty() {
            let iterator = unsafe { self.waiting_nodes.iter() };

            let size = iterator.size_hint();

            let mut count = 0;

            for (node, id) in iterator {
                count += 1;
                unsafe {
                    let node = &*node.load_acquire();
                    let mut raw_usage = node.usage.load_acquire();
                    // Newcomer initialization: if usage is 0 and we have history,
                    // initialize to the running average to prevent priority inversion
                    let served = *self.total_served.get();
                    if raw_usage == 0 && served > 0 {
                        raw_usage = *self.total_usage.get() / served;
                    }
                    job_queue.push(UsageNode {
                        usage: raw_usage,
                        tie_breaker: id,
                        node,
                        pass_entered: current_pass,
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
                    // Anti-starvation: if this node has been waiting too many
                    // passes, clamp its usage to the current queue minimum so
                    // it gets served promptly.
                    if current_pass - current.pass_entered > STARVATION_THRESHOLD {
                        if let Some(min_node) = job_queue.peek() {
                            current.usage = current.usage.min(min_node.usage);
                        }
                    }

                    // alternatively we can potentially save one __rdtscp by using `end` here
                    // which would result in a slightly inaccurate usage
                    begin = __rdtscp(&mut aux);

                    node.data.get().write(MaybeUninit::new((self.delegate)(
                        self.data.get().as_mut().unwrap_unchecked(),
                        node.data.get().read().assume_init(),
                    )));

                    let end = __rdtscp(&mut aux);
                    let cs_time = end - begin;

                    current.usage += cs_time;

                    // Track running average for newcomer initialization
                    *self.total_usage.get() += cs_time;
                    *self.total_served.get() += 1;

                    node.complete.store(true, Release);

                    // Re-insert with reset pass counter
                    current.pass_entered = current_pass;
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

            (*self.local_node.get().unwrap().get()).combiner_time_stat += end - begin;
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

        node.data = SyncUnsafeCell::new(MaybeUninit::new(data));
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

        unsafe { node.data.get().read().assume_init() }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        unsafe { self.local_node.get().map(|x| (*x.get()).combiner_time_stat) }
    }
}
