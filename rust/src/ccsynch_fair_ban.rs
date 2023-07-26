use core::panic;
use crossbeam::{
    atomic::AtomicConsume,
    utils::{Backoff, CachePadded},
};
use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    mem::transmute,
    num::*,
    ptr::null_mut,
    sync::atomic::{compiler_fence, AtomicBool, AtomicI32, AtomicI64, AtomicPtr, Ordering::*},
};
use thread_local::ThreadLocal;

use linux_futex::{Futex, Private};

use crate::dlock::DLockDelegate;
use crate::{dlock::DLock, guard::DLockGuard, syncptr::SyncMutPtr};

use self::node::Node;

mod node;

struct ThreadData<T> {
    banned_until: i64,
    node: AtomicPtr<Node<T>>,
}

pub struct CCBan<T> {
    data: SyncUnsafeCell<T>,
    tail: AtomicPtr<Node<T>>,
    local_node: ThreadLocal<SyncUnsafeCell<ThreadData<T>>>,
    avg_cs: AtomicI64,
    num_exec: AtomicI64,
    num_waiting_thread: AtomicI32,
}

impl<T> DLock<T> for CCBan<T> {
    fn lock<'a>(&self, f: impl DLockDelegate<T> + 'a) {
        self.lock(f);
    }

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<NonZeroI64> {
        let count = unsafe {
            (*((*self.local_node.get().unwrap().get()).node.load_consume())).combiner_time_stat
        };
        NonZeroI64::new(count)
    }
}

impl<T> CCBan<T> {
    pub fn new(t: T) -> CCBan<T> {
        let node = Box::into_raw(Box::new(Node::new()));
        CCBan {
            data: SyncUnsafeCell::new(t),
            tail: AtomicPtr::from(node),
            local_node: ThreadLocal::new(),
            avg_cs: AtomicI64::default(),
            num_exec: AtomicI64::default(),
            num_waiting_thread: AtomicI32::default(),
        }
    }

    fn ban(&self, data: &mut ThreadData<T>, current_cs: u64) {
        let mut aux = 0;
        unsafe {
            let now = __rdtscp(&mut aux);
            let panelty = ((current_cs) as i64) * (*self.num_waiting_thread.as_ptr() as i64)
                - self.avg_cs.load_consume() * 2;

            // println!(
            //     "current cs {}; avg cs {}",
            //     current_cs,
            //     self.avg_cs.load_consume()
            // );

            data.banned_until = (now as i64) + panelty;
        }
    }

    fn execute_fn(&self, node: &mut Node<T>, f: &mut (impl DLockDelegate<T> + ?Sized)) {
        let mut aux = 0;
        let cs_begin = unsafe { __rdtscp(&mut aux) };

        let guard = DLockGuard::new(&self.data);
        f.apply(guard);

        let cs_end = unsafe { __rdtscp(&mut aux) };

        node.completed.store(true, Release);
        node.wait.store(false, Release);

        let cs = cs_end - cs_begin;
        node.current_cs = cs;
        let num_exec = self.num_exec.fetch_add(1, Release) + 1;
        let avg_cs = self.avg_cs.load_consume();
        self.avg_cs
            .store(avg_cs + (((cs as i64) - avg_cs) / (num_exec)), Release)
    }

    pub fn lock<'a>(&self, mut f: (impl DLockDelegate<T> + 'a)) {
        let thread_data = self.local_node.get_or(|| {
            self.num_waiting_thread.fetch_add(1, Release);
            SyncUnsafeCell::new(ThreadData {
                banned_until: 0,
                node: AtomicPtr::new(Box::leak(Box::new(Node::new()))),
            })
        });
        let mut aux = 0;

        unsafe {
            let banned_until = (*thread_data.get()).banned_until;

            let backoff = Backoff::default();
            loop {
                let current = __rdtscp(&mut aux) as i64;

                if current >= banned_until {
                    break;
                }
                backoff.snooze();
            }
        }

        // use thread local node as next node
        let next_node = unsafe { &mut *(*thread_data.get()).node.load_consume() };

        next_node.next.store(null_mut(), Release);
        next_node.wait.store(true, Release);
        next_node.completed.store(false, Release);

        // assign task to next node
        let current_node = unsafe { &mut *self.tail.swap(next_node, AcqRel) };

        // assign task to current node
        unsafe {
            current_node.f = Some(transmute(&mut f as *mut dyn DLockDelegate<T>)).into();
        }

        current_node.next.store(next_node, Release);

        // put current
        unsafe {
            (*(thread_data.get())).node.store(current_node, Release);
        }

        // wait for completion
        // spinning
        let backoff = Backoff::default();
        while current_node.wait.load(Acquire) {
            // can use futex in the future
            backoff.snooze();
        }

        // check for completion, if not become the combiner

        if current_node.completed.load(Acquire) {
            unsafe {
                self.ban(&mut *(thread_data.get()), current_node.current_cs);
            }
            return;
        }

        // combiner

        #[cfg(feature = "combiner_stat")]
        let begin = unsafe { __rdtscp(&mut aux) };

        unsafe {
            let f = &mut *current_node.f.take().unwrap();
            self.execute_fn(current_node, f);
            self.ban(&mut *(thread_data.get()), current_node.current_cs);
        }

        let mut tmp_node = unsafe { &mut *current_node.next.load_consume() };

        const H: i32 = 16;

        let mut counter: i32 = 0;

        let mut next_ptr = tmp_node.next.load_consume();

        while !next_ptr.is_null() {
            if counter >= H {
                break;
            }

            counter += 1;

            if tmp_node.f.is_some() {
                unsafe {
                    let f = &mut *tmp_node.f.take().unwrap();
                    self.execute_fn(tmp_node, f);
                }
            } else {
                // panic!("No function found");
            }

            tmp_node = unsafe { &mut *(next_ptr) };

            next_ptr = tmp_node.next.load_consume();
        }

        tmp_node.wait.store(false, Relaxed);

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            (*(*thread_data.get()).node.load_consume()).combiner_time_stat += (end - begin) as i64;
        }
    }
}
