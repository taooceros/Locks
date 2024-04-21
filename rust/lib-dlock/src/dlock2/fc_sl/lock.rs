use crate::dlock2::CombinerStatistics;
use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    ops::AddAssign,
    ptr,
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
    L: RawMutex,
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
    L: RawMutex,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            combiner_lock: CachePadded::new(L::INIT),
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

        let mut combine_count = 0;

        const H: usize = 64;

        loop {
            if combine_count >= H {
                break;
            }

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

                    combine_count += 1;
                }
            }
        }

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            let combiner_statistics = &mut (*self.local_node.get().unwrap().get()).combiner_stat;
            combiner_statistics.combine_time.push(end - begin);
            combiner_statistics
                .combine_size
                .entry(combine_count)
                .or_default()
                .add_assign(1)
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

        unsafe { ptr::read(node.data.get()) }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_stat(&self) -> Option<&CombinerStatistics> {
        unsafe { self.local_node.get().map(|x| &(*x.get()).combiner_stat) }
    }
}
