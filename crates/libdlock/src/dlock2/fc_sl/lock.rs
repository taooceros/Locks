use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicPtr, AtomicU64, Ordering::*},
};

use crossbeam::utils::{Backoff, CachePadded};
use crossbeam_skiplist::SkipSet;
use derivative::Derivative;
use lock_api::RawMutex;
use thread_local::ThreadLocal;

use crate::{
    dlock2::{DLock2, DLock2Delegate},
    spin_lock::RawSpinLock,
};

use super::node::Node;

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct UsageNode<I> {
    usage: u64,
    /// Secondary ordering key so that two threads with the same `usage` value
    /// (e.g. both equal to 0 when threads first enter the lock) are still
    /// considered distinct by the `SkipSet`.  We use the node's stable memory
    /// address as the tie-breaker; every thread has a unique thread-local node.
    tie_breaker: u64,
    #[derivative(PartialEq = "ignore", PartialOrd = "ignore", Ord = "ignore")]
    node: AtomicPtr<Node<I>>,
}

#[derive(Debug)]
pub struct FCSL<T, I, F, L>
where
    T: Send + Sync,
    I: Send + 'static,
    F: Fn(&mut T, I) -> I,
    L: RawMutex,
{
    combiner_lock: CachePadded<L>,
    delegate: F,
    data: SyncUnsafeCell<T>,
    jobs: SkipSet<UsageNode<I>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<I>>>,
    /// Running total of CS time across all served requests. Written only by
    /// the combiner-lock holder; read without the lock by enrolling owners.
    total_usage: AtomicU64,
    /// Running count of served requests (same access pattern as `total_usage`)
    total_served: AtomicU64,
}

impl<T, I, F, L> FCSL<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
    L: RawMutex,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            combiner_lock: CachePadded::new(L::INIT),
            delegate,
            data: SyncUnsafeCell::new(data),
            jobs: SkipSet::new(),
            local_node: ThreadLocal::new(),
            total_usage: AtomicU64::new(0),
            total_served: AtomicU64::new(0),
        }
    }

    fn push_node(&self, node: &Node<I>) {
        node.active.store(true, Release);

        // Relaxed is enough for `node.usage`: the owner only gets here after
        // observing `active == false` with Acquire, and every combiner write
        // to `usage` precedes its Release store of `active = false`.
        let mut usage = node.usage.load(Relaxed);
        // Newcomer initialization: if usage is 0 and we have history,
        // initialize to the running average to prevent priority inversion.
        // The totals are a lock-free snapshot of combiner-owned counters; the
        // average is only a heuristic, so Relaxed suffices.
        let served = self.total_served.load(Relaxed);
        if usage == 0 && served > 0 {
            usage = self.total_usage.load(Relaxed) / served;
            node.usage.store(usage, Relaxed);
        }

        let usage_node = UsageNode {
            usage,
            tie_breaker: node as *const Node<I> as u64,
            node: AtomicPtr::new(node as *const Node<I> as *mut Node<I>),
        };

        self.jobs.insert(usage_node);
    }

    fn push_if_unactive(&self, node: &Node<I>) {
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

        for _ in 0..H {
            let current = self.jobs.pop_front();

            if current.is_none() {
                break;
            }
            unsafe {
                let current = current.unwrap_unchecked();

                let node = &*current.node.load(Acquire);

                if !node.complete.load(Acquire) {
                    node.data.get().write(MaybeUninit::new((self.delegate)(
                        self.data.get().as_mut().unwrap_unchecked(),
                        node.data.get().read().assume_init(),
                    )));

                    let end = __rdtscp(&mut aux);
                    let cs_time = end - begin;

                    // The combiner-lock holder is the sole writer of these
                    // counters (combiners are ordered by the lock), so a
                    // Relaxed load + store is an exact increment.
                    node.usage
                        .store(node.usage.load(Relaxed) + cs_time, Relaxed);

                    // Track running average for newcomer initialization
                    self.total_usage
                        .store(self.total_usage.load(Relaxed) + cs_time, Relaxed);
                    self.total_served
                        .store(self.total_served.load(Relaxed) + 1, Relaxed);

                    begin = end;

                    node.active.store(false, Release);
                    node.complete.store(true, Release);
                } else {
                    // Stale entry: the owner re-enrolled in the window between
                    // our `active = false` and `complete = true` above, then
                    // observed completion. Dropping it without clearing
                    // `active` would make the owner's next request skip
                    // enrollment and spin forever. This entry is the node's
                    // only one (the owner enrolls only after seeing `active ==
                    // false`, and nothing clears `active` between that enroll
                    // and this pop), so clearing it cannot orphan a live
                    // enrollment; an owner that already saw `active == true`
                    // for a new request re-checks it on its next outer
                    // iteration and enrolls then. Mirrors FC-PQ's buffer drain.
                    node.active.store(false, Release);
                }
            }
        }

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            *(*self.local_node.get().unwrap().get())
                .combiner_time_stat
                .get() += end - begin;
        }
    }
}

unsafe impl<'a, T, I, F> DLock2<I> for FCSL<T, I, F, RawSpinLock>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn lock(&self, data: I) -> I {
        let node = self.local_node.get_or(|| SyncUnsafeCell::new(Node::new()));

        // Shared reference: combiners may hold `&Node` (possibly via a stale
        // queue entry) concurrently. Only this owner writes the payload, and
        // only after it has consumed the previous result.
        let node = unsafe { &*node.get() };

        unsafe { node.data.get().write(MaybeUninit::new(data)) };
        node.complete.store(false, Release);

        'outer: loop {
            self.push_if_unactive(node);

            if self.combiner_lock.try_lock() {
                unsafe {
                    self.combine();
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
        unsafe {
            self.local_node
                .get()
                .map(|x| *(*x.get()).combiner_time_stat.get())
        }
    }
}
