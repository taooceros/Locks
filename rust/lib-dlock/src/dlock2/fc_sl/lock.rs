use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    ptr::{self},
    sync::atomic::{AtomicPtr, Ordering::*},
};

use crossbeam::utils::{Backoff, CachePadded};
use crossbeam_skiplist::{SkipSet};
use derivative::Derivative;
use thread_local::ThreadLocal;

use crate::{
    dlock2::{DLock2, DLock2Delegate},
    spin_lock::RawSpinLock,
    RawSimpleLock,
};

use super::node::Node;

const CLEAN_UP_AGE: u32 = 500;

#[derive(Derivative)]
#[derivative(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct UsageNode<I> {
    usage: u64,
    #[derivative(PartialEq = "ignore", PartialOrd = "ignore", Ord = "ignore")]
    node: AtomicPtr<Node<I>>,
}

#[derive(Debug)]
pub struct FCSL<T, I, F, L>
where
    T: Send + Sync,
    I: Send + 'static,
    F: Fn(&mut T, I) -> I,
    L: RawSimpleLock,
{
    combiner_lock: CachePadded<L>,
    delegate: F,
    data: SyncUnsafeCell<T>,
    jobs: SkipSet<UsageNode<I>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<I>>>,
}

impl<T, I, F, L> FCSL<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
    L: RawSimpleLock,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            combiner_lock: CachePadded::new(L::new()),
            delegate,
            data: SyncUnsafeCell::new(data),
            jobs: SkipSet::new(),
            local_node: ThreadLocal::new(),
        }
    }

    fn push_node(&self, node: &mut Node<I>) {
        let usage = node.usage;

        let usage_node = UsageNode {
            usage,
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
        #[cfg(feature = "combiner_stat")]
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
                    node.data.get().write(
                        (self.delegate)(
                            self.data.get().as_mut().unwrap_unchecked(),
                            ptr::read(node.data.get()),
                        )
                        .into(),
                    );

                    let end = __rdtscp(&mut aux);

                    node.usage += end - begin;

                    begin = end;

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
