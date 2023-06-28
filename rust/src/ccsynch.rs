use crossbeam::utils::CachePadded;
use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    mem::transmute,
    ptr::null_mut,
    sync::atomic::{compiler_fence, AtomicBool, AtomicPtr, Ordering::*},
};
use thread_local::ThreadLocal;

use linux_futex::{Futex, Private};

use crate::dlock::DLockDelegate;
use crate::{dlock::DLock, guard::DLockGuard, syncptr::SyncMutPtr};

pub struct CCSynch<T> {
    data: SyncUnsafeCell<T>,
    tail: AtomicPtr<Node<T>>,
    local_node: ThreadLocal<SyncUnsafeCell<SyncMutPtr<Node<T>>>>,
}

struct Node<T> {
    f: CachePadded<Option<*mut dyn DLockDelegate<T>>>,
    wait: Futex<Private>,
    completed: AtomicBool,
    next: SyncMutPtr<Node<T>>,
    #[cfg(feature = "combiner_stat")]
    pub(super) combiner_time_stat: i64,
}

unsafe impl<T> Send for Node<T> {}
unsafe impl<T> Sync for Node<T> {}

impl<T> Node<T> {
    pub fn new() -> Node<T> {
        Node {
            f: None.into(),
            wait: Futex::new(1),
            completed: AtomicBool::new(false),
            next: SyncMutPtr::from(null_mut()),
            combiner_time_stat: 0,
        }
    }
}

impl<T> DLock<T> for CCSynch<T> {
    fn lock<'a>(&self, f: impl DLockDelegate<T> + 'a) {
        self.lock(f);
    }

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> i64 {
        return unsafe { (*((*self.local_node.get().unwrap().get()).ptr)).combiner_time_stat };
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

    pub fn lock<'a>(&self, mut f: (impl DLockDelegate<T> + 'a)) {
        let node_cell = self
            .local_node
            .get_or(|| SyncUnsafeCell::new(SyncMutPtr::from(Box::into_raw(Box::new(Node::new())))));

        // use thread local node as next node
        let next_node = unsafe { &mut *node_cell.get() };

        unsafe {
            (*next_node.ptr).next = SyncMutPtr::from(null_mut());
            (*next_node.ptr).wait.value.store(0, Relaxed);
            (*next_node.ptr).completed.store(false, Relaxed);
        }

        // assign task to next node
        let current_node = unsafe { &mut *self.tail.swap(next_node.ptr, SeqCst) };

        // assign task to current node
        unsafe {
            current_node.f = Some(transmute(&mut f as *mut dyn DLockDelegate<T>)).into();
        }

        // Compiler fence seems to be enough for synchronization given the argument in the orignal paper
        // TODO: might requires more test in the future
        compiler_fence(Release);
        current_node.next = (next_node.ptr).into();

        let current_node_ptr = current_node as *mut Node<T>;

        // put current
        unsafe {
            *(node_cell.get()) = SyncMutPtr::from(current_node_ptr);
        }

        current_node.wait.wait(0).unwrap_or_default();

        // wait for completion
        // spinning
        // while current_node.wait.load(Acquire) {
        //     // can use futex in the future
        //     std::hint::spin_loop();
        // }

        // check for completion, if not become the combiner

        if current_node.completed.load(Relaxed) {
            return;
        }

        // combiner

        let mut aux = 0;

        #[cfg(feature = "combiner_stat")]
        let begin = unsafe { __rdtscp(&mut aux) };

        let mut tmp_node = current_node;
        const H: i32 = 16;

        let mut counter: i32 = 0;

        let mut next_ptr = tmp_node.next;

        while !next_ptr.ptr.is_null() {
            if counter >= H {
                break;
            }

            counter += 1;

            if tmp_node.f.is_some() {
                let guard = DLockGuard::new(&self.data);
                unsafe {
                    (*tmp_node.f.take().unwrap()).apply(guard);
                }

                tmp_node.completed.store(true, Relaxed);
                // note for x86 there's no need for another fence
                compiler_fence(Release);
                tmp_node.wait.value.store(1, Relaxed);
                tmp_node.wait.wake(1);
            } else {
                // panic!("No function found");
            }

            tmp_node = unsafe { &mut *(next_ptr.ptr) };
            next_ptr = tmp_node.next;
        }

        tmp_node.wait.value.store(1, Relaxed);
        tmp_node.wait.wake(1);

        #[cfg(feature = "combiner_stat")]
        let end = unsafe { __rdtscp(&mut aux) };

        #[cfg(feature = "combiner_stat")]
        unsafe {
            (*(*node_cell.get()).ptr).combiner_time_stat += (end - begin) as i64;
        }
    }
}
