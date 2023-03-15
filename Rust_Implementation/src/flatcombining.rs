use std::{
    cell::{SyncUnsafeCell, UnsafeCell},
    ops::{Deref, DerefMut},
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering},
    },
    time::Duration, mem::transmute,
};

use std::hint::spin_loop;



use linux_futex::{Futex, Private};
use thread_local::ThreadLocal;

pub struct FcLock<T> {
    pass: AtomicI32,
    flag: AtomicBool,
    data: UnsafeCell<T>,
    head: AtomicPtr<Node<T>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<T>>>,
}

unsafe impl<T> Sync for FcLock<T> {}

pub struct FCGuard<'a, T: Sized> {
    lock: &'a FcLock<T>,
}

impl<T: Sized> Deref for FCGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: Sized> DerefMut for FCGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

struct NodePtr<T> {
    ptr: *mut Node<T>,
}

impl<T> Clone for NodePtr<T> {
    fn clone(&self) -> Self {
        NodePtr { ptr: self.ptr }
    }
}
impl<T> Copy for NodePtr<T> {}

unsafe impl<T> Sync for NodePtr<T> {}
unsafe impl<T> Send for NodePtr<T> {}

impl<T> NodePtr<T> {
    pub fn from(ptr: *mut Node<T>) -> NodePtr<T> {
        NodePtr { ptr }
    }
}

struct NodeData<T> {
    age: i32,
    active: bool,
    f: Option<*mut (dyn FnMut(&mut FCGuard<T>))>,
    waiter: Futex<Private>, // id: i32,
}

unsafe impl<T> Sync for NodeData<T> {}

unsafe impl<T> Send for NodeData<T> {}

struct Node<T> {
    value: SyncUnsafeCell<NodeData<T>>,
    next: NodePtr<T>,
}


impl<T: Send + Sync> FcLock<T> {
    pub fn new(t: T) -> FcLock<T> {
        FcLock {
            pass: AtomicI32::new(0),
            flag: AtomicBool::new(false),
            data: UnsafeCell::new(t),
            head: AtomicPtr::default(),
            local_node: ThreadLocal::new(),
        }
    }

    pub fn lock<'b>(&self, f: &mut (dyn FnMut(&mut FCGuard<T>) + 'b)) {
        // static mut ID: AtomicI32 = AtomicI32::new(0);
        unsafe {
            let node = self
                .local_node
                .get_or(|| {
                    SyncUnsafeCell::new(Node {
                        value: SyncUnsafeCell::new(NodeData {
                            age: 0,
                            active: false,
                            f: None,
                            waiter: Futex::new(0),
                            // id: ID.fetch_add(1, Ordering::Relaxed),
                        }),
                        next: NodePtr { ptr: null_mut() },
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
                        (*node).next = NodePtr::from(current);
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
        let mut current = head.load(Ordering::Relaxed);
        while !current.is_null() {
            unsafe {
                let mut node_data = &mut *(*current).value.get();

                if let Some(fnc) = node_data.f {
                    node_data.age = pass;
                    (*fnc)(&mut FCGuard { lock: self });
                    node_data.f = None;
                    node_data.waiter.wake(1);
                }

                current = ((*current).next).ptr;
            }
        }
    }

    unsafe fn clean_unactive_node(&self, head: &AtomicPtr<Node<T>>, pass: i32) {
        let mut previous_ptr = head.load(Ordering::Relaxed);
        assert!(!previous_ptr.is_null());

        let mut current_ptr = (*previous_ptr).next.ptr;

        while !current_ptr.is_null() {
            let previous = &mut *(previous_ptr);
            let current = &mut *(current_ptr);

            let node_data = &mut *(*current).value.get();

            if node_data.age < pass - 50 {
                previous.next = current.next;
                current.next.ptr = null_mut();
                node_data.active = false;
                current_ptr = previous.next.ptr;
                continue;
            }
            previous_ptr = current_ptr;
            current_ptr = current.next.ptr;
        }
    }
}
