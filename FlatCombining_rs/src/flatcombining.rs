use std::{
    cell::{SyncUnsafeCell, UnsafeCell},
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc,
    },
};

use std::hint::spin_loop;

use intrusive_collections::intrusive_adapter;
use intrusive_collections::{LinkedList, LinkedListAtomicLink};
use thread_local::ThreadLocal;

use crate::I32Unsafe;

struct FcLockInner<T> {
    local_node: ThreadLocal<Arc<Node<T>>>,
    nodes: LinkedList<NodeAdapter<T>>,
}

pub struct FcLock<T> {
    pass: AtomicI32,
    flag: AtomicBool,
    data: UnsafeCell<T>,
    lock: SyncUnsafeCell<FcLockInner<T>>,
}

unsafe impl<T> Sync for FcLock<T> {}

pub struct FcGuard<'a, T: Sized + 'a> {
    lock: &'a FcLock<T>,
}

#[derive(Clone, Copy)]
struct NodeData<T> {
    age: i32,
    active: bool,
    f: Option<fn(FcGuard<T>)>,
}

unsafe impl<T> Sync for NodeData<T> {}

unsafe impl<T> Send for NodeData<T> {}

struct Node<T> {
    value: SyncUnsafeCell<NodeData<T>>,
    link: LinkedListAtomicLink,
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

intrusive_adapter!(NodeAdapter<T> = Arc<Node<T>> : Node<T> {link : LinkedListAtomicLink});

impl<T: Send + Sync> FcLock<T> {
    pub fn new(t: T) -> FcLock<T> {
        FcLock {
            pass: AtomicI32::new(0),
            flag: AtomicBool::new(false),
            data: UnsafeCell::new(t),
            lock: SyncUnsafeCell::new(FcLockInner {
                local_node: ThreadLocal::new(),
                nodes: LinkedList::new(NodeAdapter::new()),
            }),
        }
    }

    pub fn lock(&self, f: fn(FcGuard<T>)) {
        let inner_lock = self.lock.get();

        unsafe {
            let node = (*inner_lock).local_node.get_or(|| {
                Arc::new(Node {
                    value: SyncUnsafeCell::new(NodeData {
                        age: 0,
                        active: false,
                        f: None,
                    }),
                    link: LinkedListAtomicLink::new(),
                })
            });

            let node_data = node.as_ref().value.get();

            (*node_data).f = Some(f);

            if !((*node_data).active) {
                (*inner_lock).nodes.push_front(node.clone());
                (*node_data).active = true;
            }

            loop {
                if self.flag.load(Ordering::Acquire) {
                    for _ in 1..100 {
                        spin_loop();
                        if (*node.as_ref().value.get()).f.is_none() {
                            break;
                        }
                    }
                    if (*node.as_ref().value.get()).f.is_none() {
                        break;
                    }
                    continue;
                } else if !self.flag.swap(true, Ordering::AcqRel) {
                    // become the combiner
                    let current_pass = self.pass.fetch_add(1, Ordering::Relaxed);
                    self.scan_and_combining(&mut (*inner_lock).nodes, current_pass + 1);

                    self.flag.swap(false, Ordering::Release);

                    if (*node.as_ref().value.get()).f.is_none() {
                        break;
                    }
                }
            }
        }
    }
    fn scan_and_combining(&self, nodes: &mut LinkedList<NodeAdapter<T>>, pass: i32) {
        let mut cursor = nodes.cursor();

        cursor.move_next();

        while !cursor.is_null() {
            if cursor.is_null() {
                break;
            }

            let node = cursor.get().unwrap();

            let mut node_data = node.value.get();

            unsafe {
                if let Some(fnc) = (*node_data).f {
                    fnc(FcGuard { lock: &self });
                    (*node_data).age = pass;
                    (*node_data).f = None;
                }
            }

            cursor.move_next();
        }
    }
}
