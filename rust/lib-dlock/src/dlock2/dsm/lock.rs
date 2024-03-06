use crate::{atomic_extension::AtomicExtension, dlock2::DLock2};
use std::{
    arch::x86_64::__rdtscp,
    cell::{SyncUnsafeCell, UnsafeCell},
    hint::spin_loop,
    ptr::{self, null_mut},
    sync::atomic::{AtomicPtr, AtomicU8, Ordering::*},
};


use debug_unwraps::DebugUnwrapExt;
use thread_local::ThreadLocal;

use super::node::Node;
use crate::{dlock2::DLock2Delegate, parker::Parker};

#[derive(Debug)]
struct ThreadData<T> {
    nodes: UnsafeCell<[Node<T>; 2]>,
    toggle: AtomicU8,

    #[cfg(feature = "combiner_stat")]
    combiner_time_stat: SyncUnsafeCell<u64>,
}

#[derive(Debug)]
pub struct DSMSynch<T, I, F>
where
    F: DLock2Delegate<T, I>,
    I: Send,
{
    delegate: F,
    data: SyncUnsafeCell<T>,
    tail: AtomicPtr<Node<I>>,
    local_node: ThreadLocal<ThreadData<I>>,
}

impl<T, I, F> DSMSynch<T, I, F>
where
    F: DLock2Delegate<T, I>,
    I: Send,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: SyncUnsafeCell::new(data),
            tail: AtomicPtr::default(),
            local_node: ThreadLocal::new(),
        }
    }
}

trait AsMutPtr {
    fn as_mut_ptr(&self) -> *mut Self;
}

impl<T> AsMutPtr for Node<T> {
    fn as_mut_ptr(&self) -> *mut Node<T> {
        self as *const _ as *mut _
    }
}

const H: u32 = 64;

unsafe impl<T, I, F> DLock2<I> for DSMSynch<T, I, F>
where
    T: Send + Sync,
    F: DLock2Delegate<T, I>,
    I: Send,
{
    fn lock(&self, data: I) -> I {
        let thread_data = self.local_node.get_or(|| ThreadData {
            nodes: Default::default(),
            toggle: AtomicU8::new(0),
            combiner_time_stat: 0.into(),
        });
        let mut aux = 0;

        // toggle threadlocal toggle
        let toggled = thread_data.toggle.fetch_xor(1, AcqRel);

        unsafe {
            let myNode = &(*thread_data.nodes.get())[(1 - toggled) as usize];

            myNode.wait.store(true, Release);
            myNode.completed.store(false, Release);
            myNode.next.store(std::ptr::null_mut(), Release);

            // announce the request
            myNode.data.get().write(data);

            // insert the node into the queue
            let myPredNode = self.tail.swap(myNode.as_mut_ptr(), AcqRel);

            // if a node already exists in the list
            if !myPredNode.is_null() {
                let predNode = myPredNode.as_mut().unwrap_unchecked();

                predNode.next.store(myNode.as_mut_ptr(), Release);

                while myNode.wait.load_acquire() {
                    spin_loop();
                }

                if myNode.completed.load_acquire() {
                    return ptr::read(myNode.data.get());
                }
            }

            // combiner

            #[cfg(feature = "combiner_stat")]
            let begin = __rdtscp(&mut aux);

            let mut tmp_node = myNode;

            let mut counter: u32 = 0;

            loop {
                counter += 1;

                tmp_node.data.get().write((self.delegate)(
                    self.data.get().as_mut().unwrap_unchecked(),
                    tmp_node.data.get().read(),
                ));

                tmp_node.completed.store_release(true);
                tmp_node.wait.store_release(false);

                if tmp_node.next.load_acquire().is_null()
                    || (*tmp_node.next.load_acquire())
                        .next
                        .load_acquire()
                        .is_null()
                    || counter > H
                {
                    break;
                }

                tmp_node = tmp_node
                    .next
                    .load_acquire()
                    .as_ref()
                    .debug_unwrap_unchecked();
            }

            if tmp_node.next.load_acquire().is_null() {
                if self
                    .tail
                    .compare_exchange(tmp_node.as_mut_ptr(), null_mut(), Release, Relaxed)
                    .is_ok()
                {
                    return myNode.data.get().read();
                }

                while tmp_node.next.load_acquire().is_null() {
                    spin_loop();
                }
            }

            tmp_node
                .next
                .load_acquire()
                .as_ref()
                .unwrap_unchecked()
                .wait
                .store(false, Release);

            tmp_node.next.store(null_mut(), Release);

            #[cfg(feature = "combiner_stat")]
            {
                let end = __rdtscp(&mut aux);

                *thread_data.combiner_time_stat.get() += end - begin;
            }

            return myNode.data.get().read();
        }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        unsafe {
            self.local_node
                .get()
                .expect("No thread local node found")
                .combiner_time_stat
                .get()
                .read()
                .into()
        }
    }
}
