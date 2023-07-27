use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    mem::transmute,
    num::*,
    ptr::null_mut,
    sync::atomic::*,
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

use super::node::Node;

const CLEAN_UP_PERIOD: u32 = 50;
const CLEAN_UP_AGE: u32 = 50;

pub struct FcLock<T, L: RawSimpleLock> {
    pass: AtomicU32,
    combiner_lock: CachePadded<L>,
    data: SyncUnsafeCell<T>,
    head: AtomicPtr<Node<T>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<T>>>,
    num_waiting_threads: SyncUnsafeCell<i32>,
}

impl<T> FcLock<T, RawSpinLock> {
    pub fn new(data: T) -> Self {
        Self {
            pass: AtomicU32::new(0),
            combiner_lock: CachePadded::new(RawSpinLock::new()),
            data: SyncUnsafeCell::new(data),
            head: AtomicPtr::new(std::ptr::null_mut()),
            local_node: ThreadLocal::new(),
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

    fn combine(&self) {
        let mut current_ptr = self.head.load_consume();

        let pass = self.pass.fetch_add(1, Ordering::Relaxed);
        
        #[cfg(feature = "combiner_stat")]
        let mut aux: u32 = 0;
        #[cfg(feature = "combiner_stat")]
        let begin: u64;

        unsafe {
            begin = __rdtscp(&mut aux);
        }

        while !current_ptr.is_null() {
            let current = unsafe { &mut *(current_ptr) };

            if let Some(f) = current.f.into_inner() {
                current.age = pass;

                let begin = unsafe { __rdtscp(&mut aux) };

                unsafe {
                    (*f).apply(DLockGuard::new(&self.data));
                    current.f = None.into();
                }
            }

            current_ptr = current.next;
        }

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            (*self.local_node.get().unwrap().get()).combiner_time_stat += (end - begin) as i64;
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

impl<T> DLock<T> for FcLock<T, RawSpinLock> {
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

                self.combine();

                if self.pass.load_consume() % CLEAN_UP_PERIOD == 0 {
                    unsafe {
                        self.clean_unactive_node(&self.head, self.pass.load_consume());
                    }
                }

                self.combiner_lock.unlock();
            }

            if node.f.into_inner().is_none() {
                // combiner
                return;
            }

            if node.f.into_inner().is_some() {
                backoff.snooze();
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
