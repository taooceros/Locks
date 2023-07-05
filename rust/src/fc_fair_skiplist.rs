use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    mem::transmute,
    num::*,
    sync::atomic::Ordering::*,
    sync::atomic::*,
    thread::{current},
    time::Duration,
};

use crossbeam::{
    atomic::AtomicConsume,
    epoch::{default_collector, pin, Guard},
    utils::{Backoff, CachePadded},
};
use crossbeam_skiplist::SkipList;
use thread_local::ThreadLocal;


use crate::{
    dlock::{DLock, DLockDelegate},
    guard::DLockGuard,
    raw_spin_lock::RawSpinLock,
    RawSimpleLock,
};

use self::node::Node;

mod node;

const COMBINER_SLICE_MS: Duration = Duration::from_micros(100);
const COMBINER_SLICE: u64 = (COMBINER_SLICE_MS.as_nanos() as u64) * 2400;

pub struct FcSL<T, L: RawSimpleLock> {
    combiner_lock: CachePadded<L>,
    data: SyncUnsafeCell<T>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<T>>>,
    jobs: SkipList<u64, AtomicPtr<Node<T>>>,
}

impl<T: 'static> FcSL<T, RawSpinLock> {
    pub fn new(data: T) -> Self {
        Self {
            combiner_lock: CachePadded::new(RawSpinLock::new()),
            data: SyncUnsafeCell::new(data),
            local_node: ThreadLocal::new(),
            jobs: SkipList::new(default_collector().clone()),
        }
    }

    fn push_node(&self, node: *mut Node<T>, guard: &Guard) {
        unsafe {
            (*node).active.store(true, Release);

            self.jobs.insert(
                (*node).usage + (current().id().as_u64().get()),
                AtomicPtr::new(node),
                guard,
            );
        }
    }

    fn push_if_unactive(&self, node: &mut Node<T>, guard: &Guard) {
        if node.active.load_consume() {
            return;
        }
        self.push_node(node, guard);
    }

    fn combine(&self, guard: &Guard) {
        let mut aux = 0;
        let combiner_begin = unsafe { __rdtscp(&mut aux) };
        let mut slice: u64 = 0;

        while slice < COMBINER_SLICE {
            let begin = unsafe { __rdtscp(&mut aux) };

            let front_entry = self.jobs.pop_front(guard);

            match front_entry {
                Some(entry) => {
                    let node_ptr = entry.value();

                    let node = unsafe { &mut *node_ptr.load_consume() };

                    if let Some(f) = node.f.into_inner() {
                        unsafe {
                            (*f).apply(DLockGuard::new(&self.data));
                            node.f = None.into();
                        }
                    }
                    let end = unsafe { __rdtscp(&mut aux) };

                    let cs = end - begin;

                    slice += cs;

                    node.usage += cs;

                    node.active.store(false, Release);
                }
                // No additional jobs
                None => break,
            }
        }
        let end = unsafe { __rdtscp(&mut aux) };

        #[cfg(feature = "combiner_stat")]
        unsafe {
            (*self.local_node.get().unwrap().get()).combiner_time_stat +=
                (end - combiner_begin) as i64;
        }
    }
}

impl<T: 'static> DLock<T> for FcSL<T, RawSpinLock> {
    fn lock<'a>(&self, mut f: (impl DLockDelegate<T> + 'a)) {
        let node_cell = self.local_node.get_or(|| SyncUnsafeCell::new(Node::new()));

        let node = unsafe { &mut *(node_cell).get() };

        let guard = &pin();

        // it is supposed to consume the function before return, so it should be safe to erase the lifetime
        node.f = unsafe { Some(transmute(&mut f as *mut dyn DLockDelegate<T>)).into() };

        self.push_if_unactive(node, guard);

        let backoff = Backoff::new();

        loop {
            if self.combiner_lock.try_lock() {
                // combiner

                self.combine(guard);
                
                self.combiner_lock.unlock();
            }

            if node.f.into_inner().is_none() {
                node.active.store(false, Release);
                return;
            }

            if node.f.into_inner().is_some() {
                backoff.snooze();
            }

            self.push_if_unactive(node, guard);
        }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<NonZeroI64> {
        let count = unsafe { (*self.local_node.get().unwrap().get()).combiner_time_stat };
        NonZeroI64::new(count)
    }
}
