use std::{
    arch::x86_64::__rdtscp, cell::SyncUnsafeCell, cmp::max, hint, mem::transmute, num::*,
    ptr::null_mut, sync::atomic::*, time::Duration,
};

use crossbeam::{
    atomic::AtomicConsume,
    utils::{Backoff, CachePadded},
};
use thread_local::ThreadLocal;

use crate::{
    dlock::{DLock, DLockDelegate},
    guard::DLockGuard,
    spin_lock::RawSpinLock,
    RawSimpleLock,
};

use self::node::Node;

mod node;

const CLEAN_UP_PERIOD: u32 = 50;
const CLEAN_UP_AGE: u32 = 50;
const COMBINER_SLICE_MS: Duration = Duration::from_micros(100);
const COMBINER_SLICE: i64 = (COMBINER_SLICE_MS.as_nanos() as i64) * 2400;

pub struct FcFairBanSliceLock<T, L: RawSimpleLock> {
    pass: AtomicU32,
    combiner_lock: CachePadded<L>,
    data: SyncUnsafeCell<T>,
    head: AtomicPtr<Node<T>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<T>>>,
    avg_cs: SyncUnsafeCell<i64>,
    avg_combiner_slice: SyncUnsafeCell<i64>,
    num_exec: SyncUnsafeCell<i32>,
    num_waiting_threads: SyncUnsafeCell<i32>,
}

impl<T> FcFairBanSliceLock<T, RawSpinLock> {
    pub fn new(data: T) -> Self {
        Self {
            pass: AtomicU32::new(1),
            combiner_lock: CachePadded::new(RawSpinLock::new()),
            data: SyncUnsafeCell::new(data),
            head: AtomicPtr::new(std::ptr::null_mut()),
            local_node: ThreadLocal::new(),
            avg_cs: SyncUnsafeCell::new(0),
            avg_combiner_slice: SyncUnsafeCell::new(0),
            num_exec: SyncUnsafeCell::new(0),
            num_waiting_threads: SyncUnsafeCell::new(0),
        }
    }

    fn push_node(&self, node: &mut Node<T>) {
        let mut head = self.head.load(Ordering::Acquire);
        loop {
            node.next = head;
            match self
                .head
                .compare_exchange_weak(head, node, Ordering::Release, Ordering::Acquire)
            {
                Ok(_) => {
                    node.active.store(true, Ordering::Release);
                    break;
                }
                Err(x) => head = x,
            }
        }
    }

    fn push_if_unactive(&self, node: &mut Node<T>) {
        if node.active.load_consume() {
            return;
        }
        self.push_node(node);
    }

    fn combine(&self, combiner_node: &mut Node<T>) {
        let mut current_ptr = self.head.load_consume();

        let pass = self.pass.fetch_add(1, Ordering::Relaxed);
        let mut aux: u32 = 0;

        #[cfg(feature = "combiner_stat")]
        let combine_begin = unsafe { __rdtscp(&mut aux) };
        #[cfg(feature = "combiner_stat")]
        let mut combine_end: u64 = combine_begin;

        let mut already_work = combiner_node.combiner_time;

        while !current_ptr.is_null() {
            let current = unsafe { &mut *(current_ptr) };

            if let Some(f) = current.f.into_inner() {
                current.age = pass;

                let begin = unsafe { __rdtscp(&mut aux) };

                if current.banned_until > begin {
                    current_ptr = current.next;
                    continue;
                }

                unsafe {
                    (*f).apply(DLockGuard::new(&self.data));
                    current.f = None.into();
                }

                let end = unsafe { __rdtscp(&mut aux) };

                let cs = (end - begin) as i64;

                unsafe {
                    let avg_cs = &mut *self.avg_cs.get();
                    let num_exec = &mut *self.num_exec.get();
                    let num_waiting_threads = *self.num_waiting_threads.get();

                    *num_exec += 1;
                    *avg_cs = *avg_cs + ((cs - *avg_cs) / (*num_exec as i64));
                    current.banned_until =
                        end + (max((cs * (num_waiting_threads as i64)) - *avg_cs, 0) as u64);
                }

                already_work += cs;

                // if already_work > COMBINER_SLICE {
                //     combiner_node.combiner_time_stat += (end - combine_begin) as i64;
                //     return;
                // }

                combine_end = end;
            }

            current_ptr = current.next;
        }

        combiner_node.combiner_time = already_work;
        combiner_node.combiner_time_stat += (combine_end - combine_begin) as i64;

        unsafe {
            let avg_combiner_slice = &mut *self.avg_combiner_slice.get();
            *avg_combiner_slice =
                *avg_combiner_slice + ((already_work - *avg_combiner_slice) / (pass as i64));
        }
    }

    unsafe fn clean_unactive_node(&self, head: &AtomicPtr<Node<T>>, pass: u32) {
        let mut previous_ptr = head.load(Ordering::Relaxed);
        debug_assert!(!previous_ptr.is_null());

        let mut current_ptr = (*previous_ptr).next;

        while !current_ptr.is_null() {
            let previous = &mut *(previous_ptr);
            let current = &mut *(current_ptr);

            if pass - current.age > CLEAN_UP_AGE {
                previous.next = current.next;
                current.next = null_mut();
                current.active.store(false, Ordering::SeqCst);
                current_ptr = previous.next;
                continue;
            }

            previous_ptr = current_ptr;
            current_ptr = current.next;
        }
    }
}

fn spin(num: i64) {
    for _ in 0..num {
        hint::spin_loop();
    }
}

impl<T> DLock<T> for FcFairBanSliceLock<T, RawSpinLock> {
    fn lock<'a>(&self, mut f: (impl DLockDelegate<T> + 'a)) {
        let node = self.local_node.get_or(|| {
            unsafe {
                (AtomicI32::from_ptr(self.num_waiting_threads.get()))
                    .fetch_add(1, Ordering::Release);
            }
            SyncUnsafeCell::new(Node::new())
        });

        let node = unsafe { &mut *(node).get() };

        self.push_if_unactive(node);

        // it is supposed to consume the function before return, so it should be safe to erase the lifetime
        node.f = unsafe { Some(transmute(&mut f as *mut dyn DLockDelegate<T>)).into() };

        node.waiter.value.store(0, Ordering::Release);

        let backoff = Backoff::new();

        loop {
            if self.combiner_lock.try_lock() {
                // combiner

                self.combine(node);

                if self.pass.load_consume() % CLEAN_UP_PERIOD == 0 {
                    unsafe {
                        self.clean_unactive_node(&self.head, self.pass.load_consume());
                    }
                }

                self.combiner_lock.unlock();
            }

            if node.f.into_inner().is_none() {
                return;
            }

            // combiner break

            unsafe {
                let avg_combiner_slice = *self.avg_combiner_slice.get();
                let _spin_factor = 1;

                backoff.snooze();

                if avg_combiner_slice != 0 {
                    for _ in 0..(node.combiner_time / (avg_combiner_slice)) {
                        backoff.snooze();
                    }
                }
            }

            self.push_if_unactive(node);
        }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<NonZeroI64> {
        let count = unsafe { (*self.local_node.get().unwrap().get()).combiner_time_stat };
        NonZeroI64::new(count)
    }
}
