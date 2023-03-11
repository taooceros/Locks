use std::{
    cell::{SyncUnsafeCell, UnsafeCell},
    ops::{Deref, DerefMut},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering},
        Arc,
    },
    thread::yield_now,
};

use std::hint::spin_loop;

use intrusive_collections::intrusive_adapter;
use intrusive_collections::LinkedListAtomicLink;
use thread_local::ThreadLocal;

pub struct FcLock<T> {
    pass: AtomicI32,
    flag: AtomicBool,
    data: UnsafeCell<T>,
    head: AtomicPtr<Node<T>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<T>>>,
}

unsafe impl<T> Sync for FcLock<T> {}

pub struct FcGuard<'a, T: Sized + 'a> {
    lock: &'a FcLock<T>,
}

struct NodePtr<T> {
    ptr: *mut Node<T>,
}

unsafe impl<T> Sync for NodePtr<T> {}
unsafe impl<T> Send for NodePtr<T> {}

impl<T> NodePtr<T> {
    pub fn from(ptr: *mut Node<T>) -> NodePtr<T> {
        NodePtr { ptr }
    }
}

#[derive(Clone, Copy)]
struct NodeData<T> {
    age: i32,
    active: bool,
    f: Option<fn(FcGuard<T>)>,
    id: i32,
}

unsafe impl<T> Sync for NodeData<T> {}

unsafe impl<T> Send for NodeData<T> {}


struct Node<T> {
    value: SyncUnsafeCell<NodeData<T>>,
    next: NodePtr<T>,
}

impl<T: Sized> Deref for FcGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: Sized> DerefMut for FcGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

intrusive_adapter!(NodeAdapter<T> = Arc<Node<T>> : Node<T> {next : LinkedListAtomicLink});

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

    pub fn lock(&self, f: fn(FcGuard<T>)) {
        static mut ID: AtomicI32 = AtomicI32::new(0);

        unsafe {
            let node = self
                .local_node
                .get_or(|| {
                    SyncUnsafeCell::new(Node {
                        value: SyncUnsafeCell::new(NodeData {
                            age: 0,
                            active: false,
                            f: None,
                            id: ID.fetch_add(1, Ordering::Relaxed),
                        }),
                        next: NodePtr { ptr: null_mut() },
                    })
                })
                .get();

            let node_data = (*node).value.get();

            (*node_data).f = Some(f);

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
                    (*node_data).active = true;
                }

                if (*node_data).f.is_none() {
                    return;
                }

                // assert!((*node_data).active);

                if self.flag.load(Ordering::Acquire) {
                    for _ in 1..100 {
                        spin_loop();
                        if (*node_data).f.is_none() {
                            return;
                        }
                    }
                    if (*node_data).f.is_none() {
                        return;
                    }
                    yield_now();
                } else if !self.flag.swap(true, Ordering::AcqRel) {
                    // become the combiner
                    let current_pass = self.pass.fetch_add(1, Ordering::Relaxed);
                    self.scan_and_combining(&(self).head, current_pass + 1);

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
                let mut node_data = (*current).value.get();

                if let Some(fnc) = (*node_data).f {
                    (*node_data).age = pass;
                    fnc(FcGuard { lock: self });
                    (*node_data).f = None;
                } else if (*node_data).age < pass - 50 {
                    
                    // (*node_data).active = false;
                    // continue;
                }

                current = ((*current).next).ptr;
            }
        }
    }
}
