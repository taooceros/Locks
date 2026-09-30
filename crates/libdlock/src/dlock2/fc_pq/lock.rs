#[cfg(not(miri))]
use std::arch::x86_64::{__rdtscp, _mm_prefetch, _MM_HINT_T0};

use derivative::Derivative;
use lock_api::RawMutex;
use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
use std::fmt::Debug;
use std::mem::MaybeUninit;
#[cfg(not(feature = "fcpq_cached_tid"))]
use std::thread::current;
use std::{
    cell::SyncUnsafeCell,
    sync::atomic::{AtomicPtr, Ordering::*},
};

use crossbeam::utils::{Backoff, CachePadded};

use thread_local::ThreadLocal;

#[cfg(any(test, feature = "combiner_pass_stat"))]
use crate::dlock2::pass_stat::{PassRecorder, PassStats};
use crate::{
    atomic_extension::AtomicExtension,
    dlock2::{DLock2, DLock2Delegate},
    sequential_priority_queue::SequentialPriorityQueue,
};

mod buffer;

use self::buffer::ConcurrentRingBuffer;

use super::node::Node;

#[cfg(all(feature = "fcpq_fast_path", feature = "fcpq_fast_path_notime"))]
compile_error!("features `fcpq_fast_path` and `fcpq_fast_path_notime` are mutually exclusive");

// Test-only hook run by `push_node` between `active=true` and the ring
// publication (`tail.fetch_add`). A fast-path gate cannot see a request in
// this window; the fast-path tests widen it (plan D5).
#[cfg(test)]
thread_local! {
    pub(crate) static ENROLL_WINDOW_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };
}

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

/// Per-pass cap on priority-queue pops in `combine()` (the pass length H).
///
/// `Fixed(n)` pops at most `n` entries per pass; `Active` pops at most as many
/// entries as the queue holds at pass start (after the announcement ring has
/// been drained into it), i.e. the number of active or enrolled nodes. A pop
/// that finds an already-completed node counts against the cap without running
/// a body, so a pass runs at most `cap` bodies, possibly fewer.
///
/// Served nodes are re-inserted and a node whose owner resubmits during the
/// pass is served again in the same pass, so a large cap lets one combiner run
/// many bodies in a row; `Active` bounds the pass to about one body per
/// enrolled node, like FC's single sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassCap {
    Fixed(usize),
    Active,
}

impl PassCap {
    /// The original constant, used by `FCPQ::new`.
    pub const DEFAULT: PassCap = PassCap::Fixed(64);
}

impl Default for PassCap {
    fn default() -> Self {
        Self::DEFAULT
    }
}

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
    /// Read-only after construction; read once per pass by the combiner.
    pass_cap: PassCap,
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
    /// Combiner-only pass counters (stats build and unit tests only).
    #[cfg(any(test, feature = "combiner_pass_stat"))]
    pass_stats: SyncUnsafeCell<PassRecorder>,
}

impl<T, I, PQ, F, L> FCPQ<T, I, PQ, F, L>
where
    T: Send + Sync,
    I: Send,
    PQ: SequentialPriorityQueue<UsageNode<'static, I>> + Debug,
    F: DLock2Delegate<T, I>,
    L: RawMutex,
{
    /// FC-PQ with the default pass cap, `PassCap::DEFAULT` (64 pops per pass).
    pub fn new(data: T, delegate: F) -> Self {
        Self::with_pass_cap(data, delegate, PassCap::DEFAULT)
    }

    /// FC-PQ with an explicit per-pass pop cap; `Fixed(0)` is rejected (a pass
    /// that can never serve would livelock every requester).
    pub fn with_pass_cap(data: T, delegate: F, pass_cap: PassCap) -> Self {
        assert!(
            pass_cap != PassCap::Fixed(0),
            "FC-PQ pass cap must be at least 1"
        );
        Self {
            combiner_lock: CachePadded::new(L::INIT),
            delegate,
            pass_cap,
            job_queue: PQ::new().into(),
            waiting_nodes: ConcurrentRingBuffer::new(),
            data: SyncUnsafeCell::new(data),
            local_node: ThreadLocal::new(),
            total_usage: SyncUnsafeCell::new(0),
            total_served: SyncUnsafeCell::new(0),
            combine_pass: SyncUnsafeCell::new(0),
            #[cfg(any(test, feature = "combiner_pass_stat"))]
            pass_stats: SyncUnsafeCell::new(PassRecorder::new()),
        }
    }

    fn push_node(&self, node: &Node<I>) {
        node.active.store(true, Release);
        #[cfg(test)]
        ENROLL_WINDOW_HOOK.with(|hook| {
            if let Some(hook) = &*hook.borrow() {
                hook();
            }
        });
        // Plan D6: the tie-breaker id is cached in the node instead of
        // cloning and dropping a `Thread` handle per enrollment.
        #[cfg(feature = "fcpq_cached_tid")]
        let tid = node.tid;
        #[cfg(not(feature = "fcpq_cached_tid"))]
        let tid = current().id().as_u64().into();
        self.waiting_nodes
            .push((AtomicPtr::new(node as *const _ as *mut Node<I>), tid));
    }

    fn push_if_unactive(&self, node: &Node<I>) {
        if node.active.load(Acquire) {
            return;
        }

        self.push_node(node);
    }

    /// Release the combiner lock; the caller must hold it. Every unlock path,
    /// the non-combining fast path included, goes through here so that E0(a)'s
    /// `spin_park` post-unlock `parked` check (`../Locks-e0`) merges as one
    /// hunk. Without `spin_park` it is just `unlock()`.
    #[inline(always)]
    fn release_combiner(&self) {
        // SAFETY: the caller holds the combiner lock.
        unsafe { self.combiner_lock.unlock() };
    }

    /// Fast-path gate (plan D2), evaluated once per request, before this
    /// request is published. True iff no request but the caller's is pending:
    /// the caller's node is in neither the ring nor the PQ, so no combiner can
    /// reach its payload; every PQ node is active, hence pending; and no ring
    /// ticket is reserved.
    ///
    /// Arrival is ring publication (plan D5): another thread B is pending from
    /// its `tail.fetch_add` in `waiting_nodes.push`, not from its earlier
    /// `active=true`. Before that increment B is invisible to the gate, so
    /// any number of fast-path CSs may run while B sits between the two
    /// stores. A gate that reads B's increment fails, as does every later
    /// gate until B is served: the ticket stays visible until drained, then
    /// B's node is in the PQ. Fast-path CSs hold the combiner lock, so once
    /// the increment is visible (on x86, when the locked `fetch_add`
    /// completes) at most one fast-path CS, already past its gate, finishes
    /// ahead of B.
    /// B's progress does not depend on the holder: B retries `try_lock` every
    /// 8 backoffs and its own combine drains its entry.
    ///
    /// # Safety
    /// The caller holds `combiner_lock` and `node` is its own ThreadLocal node.
    #[cfg(any(feature = "fcpq_fast_path", feature = "fcpq_fast_path_notime"))]
    #[inline(always)]
    unsafe fn fast_path_gate(&self, node: &Node<I>) -> bool {
        // SAFETY: only this owner sets `active`; only lock holders clear it,
        // with release before their unlock, so under the lock the load is
        // exact. The queue and the ring's `head` change only under the lock,
        // so peek and `head` are exact; `tail` can only lag, as stated above.
        !node.active.load(Acquire)
            && (*self.job_queue.get()).peek().is_none()
            && self.waiting_nodes.empty()
    }

    /// Serve the caller's own, never published request under the combiner
    /// lock, charge it as a one-node combine pass would (plan D4), and
    /// release the lock.
    ///
    /// # Safety
    /// The caller holds `combiner_lock`, `fast_path_gate(node)` returned true
    /// under it, `node` is the caller's ThreadLocal node, and `input` has not
    /// been published in the node.
    #[cfg(any(feature = "fcpq_fast_path", feature = "fcpq_fast_path_notime"))]
    #[inline(always)]
    unsafe fn run_fast_path(&self, node: &Node<I>, input: I) -> I {
        // SAFETY: the request was never published and the gate saw the node
        // inactive under the lock, so no combiner holds the node and only this
        // owner touches its usage. The lock gives exclusive access to data and
        // the totals.
        let data = self.data.get().as_mut().unwrap_unchecked();

        #[cfg(feature = "fcpq_fast_path")]
        let output = {
            let mut aux: u32 = 0;
            let begin = timestamp(&mut aux);
            let output = (self.delegate)(data, input);
            let end = timestamp(&mut aux);
            let cs_time = end - begin;

            // Same charge as combine()'s ring drain plus one serve: newcomer
            // initialization, then the CS. The deactivating combiner released
            // `usage` before its unlock; the next combiner to drain this node
            // acquires it through the lock and the ring.
            let served = *self.total_served.get();
            let mut usage = node.usage.load_acquire();
            if usage == 0 && served > 0 {
                usage = *self.total_usage.get() / served;
            }
            node.usage.store_release(usage + cs_time);
            *self.total_usage.get() += cs_time;
            *self.total_served.get() = served + 1;

            // SAFETY: owner-only statistic, as in combine(). The holder is its
            // own combiner for this one-request pass.
            #[cfg(feature = "combiner_stat")]
            {
                *node.combiner_time_stat.get() += cs_time;
            }
            output
        };

        // Ablation only: no timestamps and no usage charge, so FC-PQ ranks
        // fast-path users wrongly once contention appears.
        #[cfg(feature = "fcpq_fast_path_notime")]
        let output = (self.delegate)(data, input);

        // Single writer: a plain load+store, no locked RMW.
        #[cfg(feature = "fcpq_fast_path_stat")]
        node.fast_path_hits
            .store(node.fast_path_hits.load(Relaxed) + 1, Relaxed);

        // SAFETY: lock held; a fast-path request is a one-body pass by the
        // calling thread (the statistic is combiner-only).
        #[cfg(any(test, feature = "combiner_pass_stat"))]
        self.record_pass(1, 1);

        self.release_combiner();
        output
    }

    /// Fast-path hits of the calling thread; `None` before its first `lock`.
    /// Per-thread like `get_combine_time`; always 0 without a fast-path
    /// feature.
    #[cfg(feature = "fcpq_fast_path_stat")]
    pub fn get_fast_path_hits(&self) -> Option<u64> {
        // SAFETY: a shared reference to a stable ThreadLocal node, as held by
        // combiners; the counter is atomic.
        self.local_node
            .get()
            .map(|node| unsafe { &*node.get() }.fast_path_hits.load(Relaxed))
    }

    /// Fast-path hits summed over every thread that has called `lock`. Each
    /// counter has one writer, so the sum is exact once those calls have
    /// returned and their threads are joined; concurrently it is a snapshot.
    #[cfg(feature = "fcpq_fast_path_stat")]
    pub fn fast_path_hits(&self) -> u64
    where
        I: Sync,
    {
        // SAFETY: as in get_fast_path_hits.
        self.local_node
            .iter()
            .map(|node| unsafe { &*node.get() }.fast_path_hits.load(Relaxed))
            .sum()
    }

    /// Combiner identity for the pass statistics: the address of the calling
    /// thread's ThreadLocal node (stable, nonzero, unique per live thread).
    #[cfg(any(test, feature = "combiner_pass_stat"))]
    fn record_pass(&self, bodies: usize, cap: usize) {
        let combiner = self.local_node.get().map_or(1, |node| node.get() as usize);
        // SAFETY: only the combiner-lock holder calls this.
        unsafe { (*self.pass_stats.get()).record(combiner, bodies, cap) };
    }

    /// Snapshot of the pass counters, taken under the combiner lock (so it is
    /// consistent even while other threads are combining; it waits for the
    /// current pass). Must not be called from inside a delegate, which already
    /// holds the lock.
    #[cfg(any(test, feature = "combiner_pass_stat"))]
    pub fn pass_stats(&self) -> PassStats {
        self.combiner_lock.lock();
        // SAFETY: the combiner lock is held.
        let stats = unsafe { self.pass_stats_locked() };
        // SAFETY: locked just above.
        unsafe { self.combiner_lock.unlock() };
        stats
    }

    /// # Safety
    /// The caller holds `combiner_lock`.
    #[cfg(any(test, feature = "combiner_pass_stat"))]
    unsafe fn pass_stats_locked(&self) -> PassStats {
        (*self.pass_stats.get()).snapshot()
    }

    fn combine(&self) {
        let mut aux: u32 = 0;
        #[cfg(feature = "combiner_stat")]
        let pass_begin = timestamp(&mut aux);

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

        // The pass length: fixed, or the entries the queue holds now.
        let cap = match self.pass_cap {
            PassCap::Fixed(cap) => cap,
            PassCap::Active => job_queue.len(),
        };
        #[cfg(any(test, feature = "combiner_pass_stat"))]
        let mut bodies = 0_usize;

        let mut buffer = ConstGenericRingBuffer::<UsageNode<I>, 4>::new();

        // SAFETY: combiner exclusion permits one queue/state writer; acquire
        // of complete=false observes each owner's initialized payload. Each
        // input is moved once, complete=true release returns the result.
        unsafe {
            for _ in 0..cap {
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
                    #[cfg(any(test, feature = "combiner_pass_stat"))]
                    {
                        bodies += 1;
                    }

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

        #[cfg(any(test, feature = "combiner_pass_stat"))]
        self.record_pass(bodies, cap);

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

        // Plan D1: try the lock before publishing, and bypass the PQ only if
        // the gate finds nothing else pending. The request is published only
        // after the gate fails. The node may still be enrolled from an
        // earlier call; a combiner that holds it serves any published request
        // and may then deactivate the node, so a gate after publication could
        // pass and run the request twice. `first_try` hands this CAS to the
        // loop's first iteration.
        #[cfg(any(feature = "fcpq_fast_path", feature = "fcpq_fast_path_notime"))]
        let mut first_try = if !self.combiner_lock.try_lock() {
            Some(false)
        } else if unsafe { self.fast_path_gate(node) } {
            // SAFETY: lock held, gate passed, `data` not yet published.
            return unsafe { self.run_fast_path(node, data) };
        } else if node.active.load(Acquire) {
            // Still enrolled from an earlier call, so the push below is a
            // no-op: publish and combine while still holding the lock.
            Some(true)
        } else {
            // Enrolling may spin on a full ring, which only a lock holder
            // drains: never enroll while holding the combiner lock.
            self.release_combiner();
            None
        };

        unsafe { node.data.get().write(MaybeUninit::new(data)) };
        node.complete.store(false, Release);

        'outer: loop {
            self.push_if_unactive(node);

            #[cfg(any(feature = "fcpq_fast_path", feature = "fcpq_fast_path_notime"))]
            let acquired = match first_try.take() {
                Some(acquired) => acquired,
                None => self.combiner_lock.try_lock(),
            };
            #[cfg(not(any(feature = "fcpq_fast_path", feature = "fcpq_fast_path_notime")))]
            let acquired = self.combiner_lock.try_lock();

            if acquired {
                self.combine();

                self.release_combiner();

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

// Plan D4: a fast-path request is charged exactly what a one-node combine pass
// charges, so these expectations are shared by the baseline and every timed
// build; only fcpq_fast_path_notime (fairness-incorrect by design) differs.
#[cfg(test)]
mod accounting_tests {
    use std::{cmp::Reverse, collections::BinaryHeap, sync::Barrier, thread};

    use super::*;
    use crate::spin_lock::RawSpinLock;

    type Add = fn(&mut u64, u64) -> u64;
    type Lock = FCPQ<u64, u64, BinaryHeap<Reverse<UsageNode<'static, u64>>>, Add, RawSpinLock>;

    const TIMED: bool = !cfg!(feature = "fcpq_fast_path_notime");
    const REQUESTS: u64 = if cfg!(miri) { 16 } else { 1_000 };

    fn add(counter: &mut u64, input: u64) -> u64 {
        *counter += input;
        *counter
    }

    /// (total_usage, total_served); the caller ensures no call is in flight.
    fn totals(lock: &Lock) -> (u64, u64) {
        // SAFETY: quiescent lock, so no combiner accesses the totals.
        unsafe { (*lock.total_usage.get(), *lock.total_served.get()) }
    }

    /// The calling thread's stored usage. Alone, its node ends every request
    /// inactive (fast path, or served and deactivated in the same pass), so
    /// `node.usage` is authoritative.
    fn own_usage(lock: &Lock) -> u64 {
        // SAFETY: shared access to this thread's stable node.
        unsafe { &*lock.local_node.get().unwrap().get() }
            .usage
            .load_acquire()
    }

    #[test]
    fn solo_requests_charge_usage_and_totals() {
        let lock = Lock::new(0, add);
        for expected in 1..=REQUESTS {
            assert_eq!(lock.lock(1), expected);
        }
        let (total_usage, total_served) = totals(&lock);
        if TIMED {
            assert_eq!(total_served, REQUESTS);
            assert!(total_usage > 0);
            assert_eq!(own_usage(&lock), total_usage);
        } else {
            assert_eq!((total_usage, total_served, own_usage(&lock)), (0, 0, 0));
        }
    }

    #[test]
    fn newcomer_starts_at_running_average() {
        let lock = Lock::new(0, add);
        let barrier = Barrier::new(2);
        let (before, newcomer, after) = thread::scope(|scope| {
            // The first thread stays alive while the newcomer (this thread)
            // runs: thread_local recycles an exited thread's slot, which would
            // hand the newcomer that thread's node and usage.
            scope.spawn(|| {
                for _ in 0..REQUESTS {
                    lock.lock(1);
                }
                barrier.wait();
                barrier.wait();
            });
            barrier.wait();
            let before = totals(&lock);
            let response = lock.lock(1);
            let newcomer = own_usage(&lock);
            let after = totals(&lock);
            barrier.wait();
            assert_eq!(response, REQUESTS + 1);
            (before, newcomer, after)
        });
        let ((usage_before, served_before), (usage_after, served_after)) = (before, after);
        if TIMED {
            assert_eq!(served_after, served_before + 1);
            let cs_time = usage_after - usage_before;
            assert_eq!(newcomer, usage_before / served_before + cs_time);
        } else {
            assert_eq!((usage_after, served_after, newcomer), (0, 0, 0));
        }
    }
}

// The pass length H: a pass runs at most `cap` bodies (a pop that finds a
// completed node counts against the cap without running one).
#[cfg(test)]
mod pass_cap_tests {
    use std::{cmp::Reverse, collections::BinaryHeap, thread};

    use super::*;
    use crate::dlock2::pass_stat::PassStats;
    use crate::spin_lock::RawSpinLock;

    type Add = fn(&mut u64, u64) -> u64;
    type Lock = FCPQ<u64, u64, BinaryHeap<Reverse<UsageNode<'static, u64>>>, Add, RawSpinLock>;

    const PENDING: usize = 6;
    const REQUESTS: u64 = if cfg!(miri) { 8 } else { 500 };

    fn add(counter: &mut u64, input: u64) -> u64 {
        *counter += input;
        *counter
    }

    /// Holds the combiner lock while `PENDING` threads each announce one
    /// request, runs exactly one pass, then lets the threads finish. Returns
    /// the statistics after that first pass and after everything completed.
    fn first_pass(cap: PassCap) -> (PassStats, PassStats) {
        let lock = Lock::with_pass_cap(0, add, cap);
        let first = thread::scope(|scope| {
            assert!(lock.combiner_lock.try_lock());
            // The driving thread acts as the combiner, so it needs a node of its own
            // (combiner statistics are attributed to the calling thread's node).
            lock.local_node.get_or(|| SyncUnsafeCell::new(Node::new()));
            let handles: Vec<_> = (0..PENDING).map(|_| scope.spawn(|| lock.lock(1))).collect();
            // Every ticket is reserved (the pass waits for their publication).
            while lock.waiting_nodes.tail.load(Acquire) < PENDING {
                std::hint::spin_loop();
            }
            lock.combine();
            // SAFETY: this thread holds the combiner lock (taken above).
            let first = unsafe { lock.pass_stats_locked() };
            lock.release_combiner();
            for handle in handles {
                handle.join().unwrap();
            }
            first
        });
        // SAFETY: quiescent lock.
        assert_eq!(unsafe { *lock.data.get() }, PENDING as u64);
        (first, lock.pass_stats())
    }

    #[test]
    fn default_is_the_original_cap_of_64() {
        assert_eq!(PassCap::default(), PassCap::Fixed(64));
        assert_eq!(Lock::new(0, add).pass_cap, PassCap::Fixed(64));
    }

    #[test]
    #[should_panic(expected = "pass cap must be at least 1")]
    fn zero_cap_is_rejected() {
        let _ = Lock::with_pass_cap(0, add, PassCap::Fixed(0));
    }

    #[test]
    fn a_pass_never_serves_more_than_a_fixed_cap() {
        for cap in 1..PENDING {
            let (first, all) = first_pass(PassCap::Fixed(cap));
            // More requests are pending than the cap, so the cap binds exactly.
            assert_eq!(
                (first.passes, first.bodies, first.last_cap),
                (1, cap as u64, cap as u64)
            );
            assert_eq!(all.cap_violations, 0, "cap {cap}");
            assert!(all.max_bodies <= cap as u64, "cap {cap}: {all:?}");
            assert_eq!(
                all.bodies, PENDING as u64,
                "every request served exactly once"
            );
        }
    }

    #[test]
    fn a_cap_at_or_above_the_pending_requests_serves_them_all() {
        for cap in [PENDING, 64] {
            let (first, all) = first_pass(PassCap::Fixed(cap));
            assert_eq!((first.bodies, first.last_cap), (PENDING as u64, cap as u64));
            assert_eq!(all.cap_violations, 0);
        }
    }

    #[test]
    fn active_cap_is_the_number_of_enrolled_nodes_at_pass_start() {
        let (first, all) = first_pass(PassCap::Active);
        assert_eq!(
            (first.bodies, first.last_cap),
            (PENDING as u64, PENDING as u64)
        );
        assert_eq!(all.cap_violations, 0);
    }

    /// Contended stress: every request runs exactly once and no pass exceeds
    /// its cap, whatever the interleaving.
    #[test]
    fn stress_respects_every_cap() {
        const THREADS: u64 = 4;
        for cap in [
            PassCap::Fixed(1),
            PassCap::Fixed(2),
            PassCap::Fixed(8),
            PassCap::DEFAULT,
            PassCap::Active,
        ] {
            let lock = Lock::with_pass_cap(0, add, cap);
            thread::scope(|scope| {
                for _ in 0..THREADS {
                    scope.spawn(|| {
                        for _ in 0..REQUESTS {
                            lock.lock(1);
                        }
                    });
                }
            });
            let stats = lock.pass_stats();
            // SAFETY: quiescent lock.
            assert_eq!(unsafe { *lock.data.get() }, THREADS * REQUESTS, "{cap:?}");
            assert_eq!(stats.bodies, THREADS * REQUESTS, "{cap:?}: {stats:?}");
            assert_eq!(stats.cap_violations, 0, "{cap:?}: {stats:?}");
            match cap {
                PassCap::Fixed(cap) => assert!(stats.max_bodies <= cap as u64, "{cap}: {stats:?}"),
                // At most one entry per thread is ever enrolled.
                PassCap::Active => assert!(stats.max_bodies <= THREADS, "{stats:?}"),
            }
        }
    }

    /// `pass_stats()` may be called while other threads combine: it takes the
    /// combiner lock, so every snapshot is consistent and monotone.
    #[test]
    fn snapshot_while_combining_is_consistent() {
        use std::sync::atomic::AtomicBool;
        let lock = Lock::with_pass_cap(0, add, PassCap::Active);
        let done = AtomicBool::new(false);
        thread::scope(|scope| {
            let workers: Vec<_> = (0..3)
                .map(|_| {
                    scope.spawn(|| {
                        for _ in 0..REQUESTS {
                            lock.lock(1);
                        }
                    })
                })
                .collect();
            let poller = scope.spawn(|| {
                let mut last = 0;
                while !done.load(Acquire) {
                    let stats = lock.pass_stats();
                    assert!(stats.bodies >= last);
                    assert_eq!(stats.bodies_hist.iter().sum::<u64>(), stats.passes);
                    last = stats.bodies;
                }
            });
            for worker in workers {
                worker.join().unwrap();
            }
            done.store(true, Release);
            poller.join().unwrap();
        });
        assert_eq!(lock.pass_stats().bodies, 3 * REQUESTS);
    }
}
