use crate::dlock2::{CombinerStatistics, DLock2};
use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    ops::AddAssign,
    ptr::{self, NonNull},
    sync::atomic::{AtomicPtr, Ordering::*},
};

use crossbeam::utils::Backoff;
use thread_local::ThreadLocal;

use super::node::Node;
use crate::dlock2::DLock2Delegate;

#[derive(Debug)]
struct ThreadData<T> {
    node: AtomicPtr<Node<T>>,
    combiner_stat: SyncUnsafeCell<CombinerStatistics>,
}

#[derive(Debug)]
pub struct CCSynch<T, I, F, const H: usize = 64>
where
    F: DLock2Delegate<T, I>,
{
    delegate: F,
    data: SyncUnsafeCell<T>,
    tail: AtomicPtr<Node<I>>,
    local_node: ThreadLocal<ThreadData<I>>,
}

impl<T, I, F> CCSynch<T, I, F>
where
    F: DLock2Delegate<T, I>,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: SyncUnsafeCell::new(data),
            tail: AtomicPtr::new(Box::leak(Box::new(Node::default()))),
            local_node: ThreadLocal::new(),
        }
    }
}

unsafe impl<T, I, F, const H: usize> DLock2<I> for CCSynch<T, I, F, H>
where
    T: Send + Sync,
    F: DLock2Delegate<T, I>,
{
    fn lock(&self, data: I) -> I {
        let thread_data = self.local_node.get_or(|| ThreadData {
            node: AtomicPtr::new(Box::leak(Box::new(Node::default()))),
            combiner_stat: SyncUnsafeCell::new(CombinerStatistics::default()),
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

        let _backoff = Backoff::new();

        // wait for the current node to be waked
        while current_node.wait.load(Acquire) {
            // spin
            // backoff.snooze();
            // spin_loop()
        }

        // check whether the current node is completed
        if current_node.completed.load(Acquire) {
            return unsafe { ptr::read(current_node.data.get()) };
        }

        // combiner

        #[cfg(feature = "combiner_stat")]
        let begin = unsafe { __rdtscp(&mut aux) };

        let mut tmp_node = current_node;

        let mut counter = 0;

        let mut next_ptr = NonNull::new(tmp_node.next.load(Acquire));

        while let Some(next_nonnull) = next_ptr {
            if counter >= H {
                break;
            }

            counter += 1;
            let next_node = unsafe { next_nonnull.as_ref() };

            unsafe {
                tmp_node.data.get().write((self.delegate)(
                    self.data.get().as_mut().unwrap_unchecked(),
                    tmp_node.data.get().read(),
                ));

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

            (*thread_data.combiner_stat.get())
                .combine_time
                .push(end - begin);

            (*thread_data.combiner_stat.get())
                .combine_size
                .entry(counter)
                .or_default()
                .add_assign(1);
        }

        return unsafe { ptr::read(current_node.data.get()) };
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_stat(&self) -> Option<&CombinerStatistics> {
        unsafe {
            self.local_node
                .get()
                .map(|local_node| local_node.combiner_stat.get().as_ref().unwrap())
        }
    }
}
