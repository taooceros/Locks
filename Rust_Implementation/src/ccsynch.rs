use std::{
    cell::SyncUnsafeCell,
    mem::transmute,
    ops::{Deref, DerefMut},
    sync::atomic::{
        AtomicBool, AtomicPtr,
        Ordering::{Acquire, Relaxed, Release, SeqCst},
    },
};

use thread_local::ThreadLocal;

use crate::{guard::Guard, operation::Operation};

pub struct CCSynch<T> {
    data: SyncUnsafeCell<T>,
    tail: AtomicPtr<Node<T>>,
    local_node: ThreadLocal<SyncUnsafeCell<NodePtr<T>>>,
}

struct Node<T> {
    f: Option<Operation<T>>,
    wait: AtomicBool,
    completed: AtomicBool,
    next: Option<NodePtr<T>>,
}

struct NodePtr<T> {
    ptr: *mut Node<T>,
}

unsafe impl<T> Sync for NodePtr<T> {}
unsafe impl<T> Send for NodePtr<T> {}

impl<T> Deref for NodePtr<T> {
    type Target = Node<T>;

    fn deref(&self) -> &Node<T> {
        unsafe { &*self.ptr }
    }
}

impl<T> DerefMut for NodePtr<T> {
    fn deref_mut(&mut self) -> &mut Node<T> {
        unsafe { &mut *self.ptr }
    }
}

impl<T> NodePtr<T> {
    pub fn from(ptr: *mut Node<T>) -> NodePtr<T> {
        NodePtr { ptr }
    }
}

impl<T> Clone for NodePtr<T> {
    fn clone(&self) -> Self {
        NodePtr { ptr: self.ptr }
    }
}

impl<T> Copy for NodePtr<T> {}

impl<T> Node<T> {
    pub fn new() -> Node<T> {
        Node {
            f: None,
            wait: AtomicBool::new(false),
            completed: AtomicBool::new(false),
            next: None,
        }
    }
}

impl<T> CCSynch<T> {
    pub fn new(t: T) -> CCSynch<T> {
        let node = Box::into_raw(Box::new(Node::new()));
        CCSynch {
            data: SyncUnsafeCell::new(t),
            tail: AtomicPtr::from(node),
            local_node: ThreadLocal::new(),
        }
    }

    pub fn lock<'a>(&self, f: &mut (dyn FnMut(&mut Guard<'a, T>) + 'a)) {
        let node_cell = self
            .local_node
            .get_or(|| SyncUnsafeCell::new(NodePtr::from(Box::into_raw(Box::new(Node::new())))));

        unsafe {
            // use thread local node as next node
            let next_node = &mut *node_cell.get();

            (next_node).next = None;
            (next_node).wait.store(true, Relaxed);
            (next_node).completed.store(false, Relaxed);

            // assign task to next node

            let current_node = &mut *self.tail.swap(next_node.ptr, SeqCst);

            // assign task to current node
            current_node.f = Some(Operation { f: (transmute(f)) });
            current_node.next = Some(*next_node);

            *node_cell.get() = NodePtr::from(current_node);

            // wait for completion
            while current_node.wait.load(Acquire) {
                // can use futex in the future
                std::hint::spin_loop();
            }

            // check for completion, if not become the combiner

            if current_node.completed.load(Relaxed) {
                return;
            }

            let mut tmp_node = current_node;
            const H: i32 = 16;

            let mut counter: i32 = 0;

            while let Some(next) = tmp_node.next {
                if counter >= H {
                    break;
                }

                counter += 1;

                if let Some(ref f) = tmp_node.f {
                    let mut guard = Guard::new(&self.data);
                    (*(f.f))(&mut guard);
                    tmp_node.f = None;
                    tmp_node.completed.store(true, Relaxed);
                    tmp_node.wait.store(false, Relaxed)
                } else {
                    panic!("No function found");
                }

                tmp_node = &mut *(next.ptr);
            }

            tmp_node.wait.store(false, Release);
        }
    }
}
