#[cfg(not(miri))]
use std::arch::x86_64::{__rdtscp, _mm_prefetch, _MM_HINT_T0};

use derivative::Derivative;
use lock_api::RawMutex;
use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::thread::current;
use std::{
    cell::SyncUnsafeCell,
    sync::atomic::{AtomicPtr, Ordering::*},
};

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

// Miri cannot execute x86 timing/prefetch intrinsics. These substitutes are
// only for exercising the real queue/ownership path under Miri, not for
// validating production timing or priority decisions.
#[cfg(miri)]
#[inline(always)]
fn timestamp(_aux: &mut u32) -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TICKS: AtomicU64 = AtomicU64::new(0);
    TICKS.fetch_add(1, Ordering::Relaxed)
}

#[cfg(not(miri))]
#[inline(always)]
fn timestamp(aux: &mut u32) -> u64 {
    // SAFETY: aux is a valid writable u32; production targets x86_64.
    unsafe { __rdtscp(aux) }
}

#[inline(always)]
fn prefetch_node<I>(node: &Node<I>) {
    #[cfg(not(miri))]
    // SAFETY: the node allocation stays live for the duration of this
    // combiner pass. Prefetch does not read or mutate its payload.
    unsafe {
        _mm_prefetch(node.data.get().cast::<i8>(), _MM_HINT_T0);
    }
    #[cfg(miri)]
    let _ = node;
}

/// Maximum number of combining passes a node may wait before its usage is
/// clamped to the current queue minimum.  Prevents unbounded starvation under
/// adversarial arrival patterns where one long-CS thread accumulates high usage
/// and is perpetually deprioritized by a stream of short-CS newcomers.
const STARVATION_THRESHOLD: u64 = 8;

/// A queue entry borrows a ThreadLocal node. Its internally manufactured
/// `'static` lifetime is valid only while its owning FCPQ exists: the sealed
/// built-in queues retain entries within the lock, and all calls must return
/// before the lock can be destroyed.
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
    // Dropped before local_node, after all lock() calls have returned.
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

    fn push_if_unactive(&self, node: &Node<I>) {
        if node.active.load(Acquire) {
            return;
        }

        self.push_node(node);
    }

    fn combine(&self) {
        let mut aux: u32 = 0;
        #[cfg(feature = "combiner_stat")]
        let pass_begin = timestamp(&mut aux);

        const H: usize = 64;

        // SAFETY: only the combiner mutex holder accesses the queue and
        // aggregate counters; no other thread borrows these interior values.
        let job_queue: &mut PQ = unsafe { &mut *self.job_queue.get() };

        // Advance the combining pass counter (combiner-only, no atomics needed)
        let current_pass = unsafe {
            let pass = &mut *self.combine_pass.get();
            *pass += 1;
            *pass
        };

        if !self.waiting_nodes.empty() {
            // SAFETY: combiner exclusion admits one iterator; each producer
            // release-publishes its value before the iterator takes that value.
            let iterator = unsafe { self.waiting_nodes.iter() };

            let size = iterator.size_hint();

            let mut count = 0;

            for (node, id) in iterator {
                count += 1;
                unsafe {
                    // SAFETY: the ring's release/acquire transfer publishes a
                    // stable ThreadLocal node; the sealed queues cannot leak it.
                    // The lock is dropped only after every call has returned.
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

        // SAFETY: combiner exclusion permits one queue/state writer; acquire
        // of complete=false observes each owner's initialized payload. Each
        // input is moved once, complete=true release returns the result.
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

                    // Prefetch the next waiter's data pointer into L1 while we
                    // execute the current delegate.  This hides the memory
                    // latency of loading the next request's input from a remote
                    // core's cache line.
                    if let Some(next) = job_queue.peek() {
                        prefetch_node(next.node);
                    }

                    // alternatively we can potentially save one __rdtscp by using `end` here
                    // which would result in a slightly inaccurate usage
                    let begin = timestamp(&mut aux);

                    node.data.get().write(MaybeUninit::new((self.delegate)(
                        self.data.get().as_mut().unwrap_unchecked(),
                        node.data.get().read().assume_init(),
                    )));

                    let end = timestamp(&mut aux);
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
            let end = timestamp(&mut aux);

            // SAFETY: this ThreadLocal statistic is only written/read by its
            // owner, even while other combiners retain shared &Node in the PQ.
            let node = &*self.local_node.get().unwrap().get();
            *node.combiner_time_stat.get() += end - pass_begin;
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

        // SAFETY: the ThreadLocal allocation stays stable until quiescent drop.
        // Other combiners can hold &Node across calls. Only this owner
        // initializes its payload, after taking the previous completed result;
        // release below publishes the new input without borrowing &mut Node.
        let node = unsafe { &*node.get() };
        unsafe { node.data.get().write(MaybeUninit::new(data)) };
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
                let mut count: u32 = 8;
                loop {
                    if node.complete.load(Acquire) {
                        break 'outer;
                    }
                    backoff.spin();
                    count = count.wrapping_sub(1);
                    if count == 0 {
                        continue 'outer;
                    }
                }
            }
        }

        // SAFETY: acquire of complete=true gives this owner its initialized
        // result, read once before it can publish another request.
        unsafe { node.data.get().read().assume_init() }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        // SAFETY: only the current ThreadLocal owner accesses this cell.
        unsafe {
            self.local_node
                .get()
                .map(|x| *(*x.get()).combiner_time_stat.get())
        }
    }
}
