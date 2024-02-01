use crate::dlock2::DLock2;
use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    ptr::{self, NonNull},
    sync::atomic::{AtomicPtr, Ordering::*},
};

use crossbeam::utils::Backoff;
use thread_local::ThreadLocal;

use super::node::Node;
use crate::{dlock2::DLock2Delegate, parker::Parker};

#[derive(Debug)]
pub struct ThreadData<T> {
    pub(crate) node: AtomicPtr<Node<T>>,
}

#[derive(Debug)]
pub struct CCSynch<T, F: DLock2Delegate<T>> {
    delegate: F,
    data: SyncUnsafeCell<T>,
    tail: AtomicPtr<Node<T>>,
    local_node: ThreadLocal<ThreadData<T>>,
}

impl<T, F: DLock2Delegate<T>> CCSynch<T, F> {
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: SyncUnsafeCell::new(data),
            tail: AtomicPtr::new(Box::leak(Box::new(Node::default()))),
            local_node: ThreadLocal::new(),
        }
    }
}

const H: u32 = 16;

impl<T: Send + Sync, F: DLock2Delegate<T>> DLock2<T, F> for CCSynch<T, F> {
    fn lock(&self, data: T) -> T {
        let thread_data = self.local_node.get_or(|| ThreadData {
            node: AtomicPtr::new(Box::leak(Box::new(Node::default()))),
        });
        let mut aux = 0;
        // use thread local node as next node
        let next_node = unsafe { &mut *thread_data.node.load(Acquire) };

        next_node.next.store(std::ptr::null_mut(), Release);
        next_node.wait.store(true, Release);
        next_node.completed.store(false, Release);

        let current_ptr = self.tail.swap(next_node, AcqRel);
        let current_node = unsafe { current_ptr.as_ref().unwrap_unchecked() };

        unsafe {
            *current_node.data.get() = data;
            current_node.next.store(next_node, Release);
            self.local_node
                .get()
                .unwrap_unchecked()
                .node
                .store(current_ptr, Relaxed)
        }

        let backoff = Backoff::new();

        // wait for the current node to be waked
        while current_node.wait.load(Acquire) {
            // spin
            backoff.snooze();
        }

        // check whether the current node is completed
        if current_node.completed.load(Acquire) {
            return unsafe { ptr::read(current_node.data.get()) };
        }

        // combiner

        #[cfg(feature = "combiner_stat")]
        let begin = unsafe { __rdtscp(&mut aux) };

        let mut tmp_node = current_node;

        let mut counter: u32 = 0;

        let mut next_ptr = NonNull::new(tmp_node.next.load(Acquire));

        while let Some(next_nonnull) = next_ptr {
            if counter >= H {
                break;
            }

            counter += 1;
            let next_node = unsafe { next_nonnull.as_ref() };

            unsafe {
                ptr::write(
                    tmp_node.data.get(),
                    (self.delegate)(
                        self.data.get().as_mut().unwrap_unchecked(),
                        ptr::read(tmp_node.data.get()),
                    ),
                );

                tmp_node.completed.store(true, Release);
                tmp_node.wait.store(false, Release);
            }

            tmp_node = next_node;
            next_ptr = NonNull::new(tmp_node.next.load(Acquire));
        }

        tmp_node.wait.store(false, Release);

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            (*thread_data.node.load(Acquire)).combiner_time_stat += (end - begin) as i64;
        }

        return unsafe { ptr::read(current_node.data.get()) };
    }
}
