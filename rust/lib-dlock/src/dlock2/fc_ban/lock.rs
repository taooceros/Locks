use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    cmp::max,
    ptr::{self, null_mut, NonNull},
    sync::atomic::{AtomicI64, AtomicPtr, AtomicU32, Ordering::*},
};

use crossbeam::utils::{Backoff, CachePadded};
use thread_local::ThreadLocal;

use crate::{
    dlock2::{DLock2, DLock2Delegate},
    spin_lock::RawSpinLock,
    RawSimpleLock,
};

use super::node::Node;

const CLEAN_UP_AGE: u32 = 500;

#[derive(Debug)]
pub struct FCBan<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    F: Fn(&mut T, I) -> I,
    L: RawSimpleLock,
{
    pass: AtomicU32,
    combiner_lock: CachePadded<L>,
    delegate: F,
    avg_cs: SyncUnsafeCell<i64>,
    num_exec: SyncUnsafeCell<i64>,
    num_waiting_threads: AtomicI64,
    data: SyncUnsafeCell<T>,
    head: AtomicPtr<Node<I>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<I>>>,
}

impl<T, I, F, L> FCBan<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
    L: RawSimpleLock,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            pass: AtomicU32::new(0),
            combiner_lock: CachePadded::new(L::new()),
            avg_cs: SyncUnsafeCell::new(0),
            num_exec: SyncUnsafeCell::new(0),
            num_waiting_threads: AtomicI64::new(0),
            delegate,
            data: SyncUnsafeCell::new(data),
            head: AtomicPtr::new(std::ptr::null_mut()),
            local_node: ThreadLocal::new(),
        }
    }

    fn push_node(&self, node: &mut Node<I>) {
        self.num_waiting_threads.fetch_add(1, Relaxed);
        let mut head = self.head.load(Acquire);
        node.active.store(true, Release);
        loop {
            node.next.store(head, Relaxed);
            match self
                .head
                .compare_exchange_weak(head, node, Release, Acquire)
            {
                Ok(_) => {
                    break;
                }
                Err(x) => head = x,
            }
        }
    }

    fn push_if_unactive(&self, node: &mut Node<I>) {
        if node.active.load(Acquire) {
            return;
        }
        self.push_node(node);
    }

    fn combine(&self) {
        let mut current_ptr = NonNull::new(self.head.load(Acquire));

        let pass = self.pass.fetch_add(1, Relaxed);

        #[cfg(feature = "combiner_stat")]
        let mut aux: u32 = 0;
        #[cfg(feature = "combiner_stat")]
        let begin: u64;

        unsafe {
            begin = __rdtscp(&mut aux);
        }

        let mut num_exec = unsafe { self.num_exec.get().read() };
        let mut avg_cs = unsafe { self.avg_cs.get().read() };

        let mut work_begin = begin;

        while let Some(current_nonnull) = current_ptr {
            let current = unsafe { current_nonnull.as_ref() };

            if current.active.load(Acquire) && !current.complete.load(Acquire) {
                unsafe {
                    // update age even when active
                    (*current.age.get()) = pass;

                    // if banned, skip

                    if work_begin >= current.banned_until.get().read() {
                        current.data.get().write(
                            (self.delegate)(
                                self.data.get().as_mut().unwrap_unchecked(),
                                ptr::read(current.data.get()),
                            )
                            .into(),
                        );
                        current.complete.store(true, Release);

                        let work_end = __rdtscp(&mut aux);
                        let cs = (work_end - work_begin) as i64;

                        num_exec += 1;
                        avg_cs += (cs - avg_cs) / (num_exec);
                        current.banned_until.get().write(
                            work_begin
                                + (max((cs * (self.num_waiting_threads.load(Relaxed))) - avg_cs, 0)
                                    as u64),
                        );

                        work_begin = work_end;
                    }
                }
            }

            current_ptr = NonNull::new(current.next.load(Acquire));
        }

        unsafe {
            self.num_exec.get().write(num_exec);
            self.avg_cs.get().write(avg_cs);
        }

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            (*self.local_node.get().unwrap().get()).combiner_time_stat += end - begin;
        }
    }

    unsafe fn clean_unactive_node(&self, head: &AtomicPtr<Node<I>>, pass: u32) {
        let previous_ptr = NonNull::new(head.load(Acquire)).unwrap();

        let mut previous_nonnull = previous_ptr;

        let mut current_ptr = NonNull::new(*previous_nonnull.as_ref().next.as_ptr());

        while let Some(current_nonnull) = current_ptr {
            let current = current_nonnull.as_ref();
            let previous = previous_nonnull.as_ref();

            // assert!(current.active.load(Acquire));

            if pass - (*current.age.get()) > CLEAN_UP_AGE {
                (*previous.next.as_ptr()) = *current.next.as_ptr();
                (*current.next.as_ptr()) = null_mut();
                current.active.store(false, Release);
                current_ptr = NonNull::new(previous.next.load(Acquire));
                self.num_waiting_threads.fetch_sub(1, Relaxed);
                continue;
            }

            previous_nonnull = current_nonnull;
            current_ptr = NonNull::new(current.next.load(Acquire));
        }
    }
}

unsafe impl<T, I, F> DLock2<I> for FCBan<T, I, F, RawSpinLock>
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
                unsafe {
                    if self.pass.load(Relaxed) % CLEAN_UP_AGE == 0 {
                        self.clean_unactive_node(&self.head, self.pass.load(Relaxed));
                    }
                }
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
                .as_ref()
                .unwrap()
                .combiner_time_stat
                .into()
        }
    }
}
