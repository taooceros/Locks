use std::{
    cell::SyncUnsafeCell,
    mem::transmute,
    sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering},
    time::Duration,
};

use crate::{dlock::DLock, guard::*, syncptr::SyncMutPtr};
use std::hint::spin_loop;

use linux_futex::Futex;
use lockfree::tls::ThreadLocal;

pub struct FcLock<T> {
    pass: AtomicI32,
    flag: AtomicBool,
    data: SyncUnsafeCell<T>,
    head: AtomicPtr<Node<T>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<T>>>,
}

mod node;
use self::node::*;

impl<T> DLock<T> for FcLock<T> {
    fn lock<'b>(&self, f: &mut (dyn FnMut(&mut Guard<T>) + 'b)) {
        self.lock(f);
    }
}

impl<T> FcLock<T> {
    pub fn new(t: T) -> FcLock<T> {
        FcLock {
            pass: AtomicI32::new(0),
            flag: AtomicBool::new(false),
            data: SyncUnsafeCell::new(t),
            head: AtomicPtr::default(),
            local_node: ThreadLocal::new(),
        }
    }

    pub fn lock<'b>(&self, f: &mut (dyn FnMut(&mut Guard<T>) + 'b)) {
        // static mut ID: AtomicI32 = AtomicI32::new(0);
        unsafe {
            let node = self
                .local_node
                .with_init(|| {
                    SyncUnsafeCell::new(Node {
                        value: SyncUnsafeCell::new(NodeData {
                            age: 0,
                            active: false,
                            f: None,
                            waiter: Futex::new(0),
                            // id: ID.fetch_add(1, Ordering::Relaxed),
                        }),
                        next: None,
                    })
                })
                .get();

            let node_data = &mut *(*node).value.get();

            node_data.waiter.value.store(0, Ordering::Relaxed);

            // it is supposed to consume the function before return, so it should be safe to erase the lifetime
            node_data.f = Some(transmute(f));

            loop {
                if !((*node_data).active) {
                    // println!("insert {}", COUNTER.fetch_add(1, Ordering::AcqRel));
                    let mut current = self.head.load(Ordering::Acquire);
                    loop {
                        (*node).next = if current.is_null() {
                            None
                        } else {
                            Some(current.into())
                        };
                        match self.head.compare_exchange_weak(
                            current,
                            node,
                            Ordering::Release,
                            Ordering::Relaxed,
                        ) {
                            Ok(_) => break,
                            Err(x) => current = x,
                        }
                    }
                    node_data.active = true;
                }

                if node_data.f.is_none() {
                    return;
                }

                // assert!((*node_data).active);

                if self.flag.load(Ordering::Acquire) {
                    for _ in 1..100 {
                        spin_loop();
                        if node_data.f.is_none() {
                            return;
                        }
                    }
                    _ = node_data.waiter.wait_for(0, Duration::from_millis(100));
                    if node_data.f.is_none() {
                        return;
                    }
                } else if !self.flag.swap(true, Ordering::AcqRel) {
                    // become the combiner
                    let current_pass = self.pass.fetch_add(1, Ordering::Relaxed);
                    self.scan_and_combining(&self.head, current_pass + 1);
                    self.clean_unactive_node(&self.head, current_pass + 1);

                    self.flag.swap(false, Ordering::Release);

                    if (*node_data).f.is_none() {
                        return;
                    }
                }
            }
        }
    }

    fn scan_and_combining(&self, head: &AtomicPtr<Node<T>>, pass: i32) {
        let head_ptr = head.load(Ordering::Relaxed);
        let mut current_opt = if head_ptr.is_null() {
            None
        } else {
            Some(SyncMutPtr::from(head_ptr))
        };

        while let Some(current) = current_opt {
            let current = unsafe { &mut *current.ptr };
            unsafe {
                let mut node_data = &mut *current.value.get();

                if let Some(fnc) = node_data.f {
                    node_data.age = pass;
                    (*fnc)(&mut Guard::new(&self.data));
                    node_data.f = None;
                    node_data.waiter.value.store(1, Ordering::Relaxed);
                    node_data.waiter.wake(1);
                }

                current_opt = current.next;
            }
        }
    }

    unsafe fn clean_unactive_node(&self, head: &AtomicPtr<Node<T>>, pass: i32) {
        let mut previous_ptr = SyncMutPtr::from(head.load(Ordering::Relaxed));
        assert!(!previous_ptr.ptr.is_null());

        let mut current_opt = (*previous_ptr.ptr).next;

        while let Some(current_ptr) = current_opt {
            let previous = &mut *(previous_ptr.ptr);
            let current = &mut *(current_ptr.ptr);

            let node_data = &mut *(*current).value.get();

            if node_data.age < pass - 50 {
                previous.next = current.next;
                current.next = None;
                node_data.active = false;
                current_opt = previous.next;
                continue;
            }
            previous_ptr = current_ptr;
            current_opt = current.next;
        }
    }
}
