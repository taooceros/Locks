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

use crate::{dlock::DLock, guard::DLockGuard, syncptr::SyncMutPtr};
use crate::{dlock::DLockDelegate, parker::Parker};

use self::node::Node;

mod node;

struct ThreadData<T, W: Parker> {
    banned_until: i64,
    node: AtomicPtr<Node<T, W>>,
}

pub struct CCSynch<T, W: Parker> {
    data: SyncUnsafeCell<T>,
    tail: AtomicPtr<Node<T, W>>,
    local_node: ThreadLocal<SyncUnsafeCell<ThreadData<T, W>>>,
}

impl<T, W: Parker> DLock<T> for CCSynch<T, W> {
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

impl<T, W: Parker> CCSynch<T, W> {
    pub fn new(t: T) -> CCSynch<T, W> {
        let node = Box::leak(Box::new(Node::<T, W>::new()));
        node.wait.wake();
        CCSynch {
            data: SyncUnsafeCell::new(t),
            tail: AtomicPtr::from(node as *mut Node<T, W>),
            local_node: ThreadLocal::new(),
        }
    }

    fn execute_fn(&self, node: &mut Node<T, W>, f: &mut (impl DLockDelegate<T> + ?Sized)) {
        let guard = DLockGuard::new(&self.data);
        f.apply(guard);

        node.completed.store(true, Release);
        node.wait.wake();
    }

    pub fn lock<'a>(&self, mut f: (impl DLockDelegate<T> + 'a)) {
        let thread_data = self.local_node.get_or(|| {
            SyncUnsafeCell::new(ThreadData {
                banned_until: 0,
                node: AtomicPtr::new(Box::leak(Box::new(Node::new()))),
            })
        });
        let mut aux = 0;
        // use thread local node as next node
        let next_node = unsafe { &mut *(*thread_data.get()).node.load_consume() };

        next_node.next.store(null_mut(), Release);
        next_node.wait.reset();
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

        current_node.wait.wait();

        // check for completion, if not become the combiner

        if current_node.completed.load(Acquire) {
            return;
        }

        // combiner

        #[cfg(feature = "combiner_stat")]
        let begin = unsafe { __rdtscp(&mut aux) };

        let mut tmp_node = current_node;

        const H: i32 = 16;

        let mut counter: i32 = 0;

        let mut next_ptr = tmp_node.next.load_consume();

        while !next_ptr.is_null() {
            if counter >= H {
                break;
            }

            counter += 1;

            unsafe {
                (*next_ptr).wait.prewake();
            }

            if tmp_node.f.is_some() {
                unsafe {
                    let f = &mut *tmp_node.f.take().unwrap();
                    self.execute_fn(tmp_node, f);
                }
            } else {
                panic!("No function found");
            }

            tmp_node = unsafe { &mut *(next_ptr) };

            next_ptr = tmp_node.next.load_consume();
        }

        tmp_node.wait.wake();

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            (*(*thread_data.get()).node.load_consume()).combiner_time_stat += (end - begin) as i64;
        }
    }
}
