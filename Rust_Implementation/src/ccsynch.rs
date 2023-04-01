use std::{
    cell::SyncUnsafeCell,
    mem::transmute,
    sync::atomic::{
        AtomicBool, AtomicPtr,
        Ordering::{Acquire, Relaxed, Release, SeqCst},
    },
};

use lockfree::tls::ThreadLocal;

use crate::{dlock::DLock, guard::Guard, operation::Operation, syncptr::SyncMutPtr};

pub struct CCSynch<T> {
    data: SyncUnsafeCell<T>,
    tail: AtomicPtr<Node<T>>,
    local_node: ThreadLocal<SyncUnsafeCell<SyncMutPtr<Node<T>>>>,
}

struct Node<T> {
    f: Option<Operation<T>>,
    wait: AtomicBool,
    completed: AtomicBool,
    next: Option<SyncMutPtr<Node<T>>>,
}

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

impl<T> DLock<T> for CCSynch<T> {
    fn lock<'b>(&self, f: &mut (dyn FnMut(&mut Guard<T>) + 'b)) {
        self.lock(f);
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

    pub fn lock<'a>(&self, f: &mut (dyn FnMut(&mut Guard<T>) + 'a)) {
        let node_cell = self.local_node.with_init(|| {
            SyncUnsafeCell::new(SyncMutPtr::from(Box::into_raw(Box::new(Node::new()))))
        });

        // use thread local node as next node
        let next_node = unsafe { &mut *node_cell.get() };

        unsafe {
            (*next_node.ptr).next = None;
            (*next_node.ptr).wait.store(true, Relaxed);
            (*next_node.ptr).completed.store(false, Relaxed);
        }

        // assign task to next node
        let current_node = unsafe { &mut *self.tail.swap(next_node.ptr, SeqCst) };

        // assign task to current node
        current_node.f = Some(Operation {
            f: (unsafe { transmute(f) }),
        });
        current_node.next = Some(*next_node);

        let current_node_ptr = current_node as *mut Node<T>;

        // put current
        unsafe {
            *(node_cell.get()) = SyncMutPtr::from(current_node_ptr);
        }

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
                unsafe {
                    (*(f.f))(&mut guard);
                }
                tmp_node.f = None;
                tmp_node.completed.store(true, Relaxed);
                tmp_node.wait.store(false, Relaxed)
            } else {
                // panic!("No function found");
            }

            tmp_node = unsafe { &mut *(next.ptr) };
        }

        tmp_node.wait.store(false, Release);
    }
}
