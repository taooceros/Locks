use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    mem::transmute,
    num::*,
    sync::atomic::*,
    sync::{atomic::Ordering::*},
    thread::current,
    time::Duration,
};

use crossbeam::utils::CachePadded;
use crossbeam_skiplist::SkipMap;
use thread_local::ThreadLocal;

use crate::{
    dlock::{DLock, DLockDelegate},
    dlock::guard::DLockGuard,
    parker::{Parker},
    spin_lock::RawSpinLock,
    RawSimpleLock,
};

use self::node::Node;

mod node;

const COMBINER_SLICE_MS: Duration = Duration::from_micros(100);
const COMBINER_SLICE: u64 = (COMBINER_SLICE_MS.as_micros() as u64) * 2400;

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Usage {
    usage: u64,
    thread_id: NonZeroU64,
}

#[derive(Debug)]
pub struct FCSL<T, L, P>
where
    L: RawSimpleLock,
    P: Parker + 'static,
{
    combiner_lock: CachePadded<L>,
    data: SyncUnsafeCell<T>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<T, P>>>,
    jobs: SkipMap<Usage, AtomicPtr<Node<T, P>>>,
}

impl<T, P> FCSL<T, RawSpinLock, P>
where
    T: 'static,
    P: Parker,
{
    pub fn new(data: T) -> Self {
        Self {
            combiner_lock: CachePadded::new(RawSpinLock::new()),
            data: SyncUnsafeCell::new(data),
            local_node: ThreadLocal::new(),
            jobs: SkipMap::new(),
        }
    }

    fn push_node(&self, node: *mut Node<T, P>) {
        unsafe {
            let key = Usage {
                usage: (*node).usage,
                thread_id: current().id().as_u64(),
            };

            self.jobs.insert(key, AtomicPtr::new(node));
        }
    }

    fn combine(&self, _combiner_node: &mut Node<T, P>) {
        // println!("{} is combining", current().name().unwrap());

        let mut aux = 0;
        let combiner_begin = unsafe { __rdtscp(&mut aux) };
        let mut slice: u64 = 0;

        while slice < COMBINER_SLICE {
            let begin = unsafe { __rdtscp(&mut aux) };

            let front_entry = self.jobs.pop_front();

            match front_entry {
                Some(entry) => {
                    let node_ptr = entry.value();

                    let node = unsafe { &mut *node_ptr.load(Acquire) };

                    unsafe {
                        node.parker.prewake();

                        assert!(node.finish.load(Acquire) == false);

                        (*node.f.expect("delegate should be found"))
                            .apply(DLockGuard::new(&self.data));
                        node.finish.store(true, Release);
                        // No need for the following but can be used to test
                        // node.f = None.into();
                    }

                    let end = unsafe { __rdtscp(&mut aux) };

                    let cs = end - begin;

                    slice += cs;

                    node.usage += cs;

                    node.parker.wake();
                }
                // No additional jobs
                None => break,
            }
        }
        let end = unsafe { __rdtscp(&mut aux) };

        let back = self.jobs.back();

        if let Some(entry) = back {
            let node_ptr = entry.value();

            let node = unsafe { &mut *node_ptr.load(Acquire) };

            node.should_combine.store(true, Release);
            node.parker.wake();
        } else {
            self.combiner_lock.unlock()
        }

        #[cfg(feature = "combiner_stat")]
        unsafe {
            (*self.local_node.get().unwrap().get()).combiner_time_stat +=
                (end - combiner_begin) as i64;
        }
    }
}

impl<T: 'static, P: Parker> DLock<T> for FCSL<T, RawSpinLock, P> {
    fn lock<'a>(&self, mut f: (impl DLockDelegate<T> + 'a)) {
        let node_cell = self.local_node.get_or(|| SyncUnsafeCell::new(Node::new()));

        let node = unsafe { &mut *(node_cell).get() };
        node.should_combine.store(false, Release);
        node.f = unsafe { Some(transmute(&mut f as *mut dyn DLockDelegate<T>)).into() };
        node.finish.store(false, Release);

        // it is supposed to consume the function before return, so it should be safe to erase the lifetime

        node.parker.reset();

        self.push_node(node);

        loop {
            if node.should_combine.load(Acquire) || self.combiner_lock.try_lock() {
                loop {
                    node.should_combine.store(false, Release);
                    self.combine(node);

                    if !node.should_combine.load(Acquire) {
                        break;
                    }
                }
            }

            if node.finish.load(Acquire) {
                break;
            }

            let _ = node.parker.wait_timeout(Duration::from_micros(100));
        }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<NonZeroI64> {
        let count = unsafe {
            (*self
                .local_node
                .get()
                .expect("should contains thread local value")
                .get())
            .combiner_time_stat
        };
        NonZeroI64::new(count)
    }
}
