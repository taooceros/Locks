use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    ptr::{self},
    sync::atomic::{AtomicPtr, Ordering::*},
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

const CLEAN_UP_AGE: u32 = 500;

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
    /// Running total of CS time across all served requests (combiner-only access)
    total_usage: SyncUnsafeCell<u64>,
    /// Running count of served requests (combiner-only access)
    total_served: SyncUnsafeCell<u64>,
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
            total_usage: SyncUnsafeCell::new(0),
            total_served: SyncUnsafeCell::new(0),
        }
    }

    fn push_node(&self, node: &mut Node<I>) {
        node.active.store(true, Release);

        let mut usage = node.usage;
        // Newcomer initialization: if usage is 0 and we have history,
        // initialize to the running average to prevent priority inversion
        unsafe {
            let served = *self.total_served.get();
            if usage == 0 && served > 0 {
                usage = *self.total_usage.get() / served;
                node.usage = usage;
            }
        }

        let usage_node = UsageNode {
            usage,
            tie_breaker: node as *const Node<I> as u64,
            node: AtomicPtr::new(node),
        };

        self.jobs.insert(usage_node);
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

        for _ in 0..H {
            let current = self.jobs.pop_front();

            if current.is_none() {
                break;
            }
            unsafe {
                let current = current.unwrap_unchecked();

                let node = &mut *current.node.load(Acquire);

                if !node.complete.load(Acquire) {
                    node.data.get().write(MaybeUninit::new((self.delegate)(
                        self.data.get().as_mut().unwrap_unchecked(),
                        node.data.get().read().assume_init(),
                    )));

                    let end = __rdtscp(&mut aux);
                    let cs_time = end - begin;

                    node.usage += cs_time;

                    // Track running average for newcomer initialization
                    *self.total_usage.get() += cs_time;
                    *self.total_served.get() += 1;

                    begin = end;

                    node.active.store(false, Release);
                    node.complete.store(true, Release);
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

unsafe impl<'a, T, I, F> DLock2<I> for FCSL<T, I, F, RawSpinLock>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn lock(&self, data: I) -> I {
        let node = self.local_node.get_or(|| SyncUnsafeCell::new(Node::new()));

        let node = unsafe { &mut *node.get() };

        node.data = SyncUnsafeCell::new(MaybeUninit::new(data));
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
        unsafe { self.local_node.get().map(|x| (*x.get()).combiner_time_stat) }
    }
}
