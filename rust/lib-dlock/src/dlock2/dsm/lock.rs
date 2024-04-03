use crate::{atomic_extension::AtomicExtension, dlock2::DLock2};
use std::{
    arch::x86_64::__rdtscp,
    cell::{SyncUnsafeCell, UnsafeCell},
    hint::spin_loop,
    mem::MaybeUninit,
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
pub struct DSMSynch<T, I, F, const H: u32 = 64>
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

unsafe impl<T, I, F, const H: u32> DLock2<I> for DSMSynch<T, I, F, H>
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

            myNode.wait.store_release(true);
            myNode.completed.store_release(false);
            myNode.next.store_release(null_mut());

            // announce the request
            myNode.data.get().write(MaybeUninit::new(data));

            // insert the node into the queue
            let myPredNode = self.tail.swap(myNode.as_mut_ptr(), AcqRel);

            // if a node already exists in the list
            if !myPredNode.is_null() {
                let predNode = myPredNode.as_mut().unwrap_unchecked();

                predNode.next.store_release(myNode.as_mut_ptr());

                while myNode.wait.load_acquire() {
                    spin_loop();
                }

                if myNode.completed.load_acquire() {
                    return myNode.data.get().read().assume_init();
                }
            }

            // combiner

            #[cfg(feature = "combiner_stat")]
            let begin = __rdtscp(&mut aux);

            let mut tmp_node = myNode;

            let mut counter: u32 = 0;

            loop {
                counter += 1;

                tmp_node.data.get().write(MaybeUninit::new((self.delegate)(
                    self.data.get().as_mut().debug_unwrap_unchecked(),
                    tmp_node.data.get().read().assume_init(),
                )));

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
                // This ordering might be wrong?
                if self
                    .tail
                    .compare_exchange(tmp_node.as_mut_ptr(), null_mut(), Acquire, Relaxed)
                    .is_ok()
                {
                    // The completed should always be true and wait should always be false because no other node is avaliable in the list
                    // It is not sure whether the acquire ordering is required because the current thread
                    // should be the combinier which means it should handle its own node.
                    if myNode.completed.load_acquire() {
                        return myNode.data.get().read().assume_init();
                    }

                    unreachable!("This should not happen");
                }

                // to wait for insertion and set the wait flag to be false
                while tmp_node.next.load_acquire().is_null() {
                    spin_loop();
                }
            }

            tmp_node
                .next
                .load_acquire()
                .as_ref()
                .debug_unwrap_unchecked()
                .wait
                .store(false, Release);

            tmp_node.next.store(null_mut(), Release);

            #[cfg(feature = "combiner_stat")]
            {
                let end = __rdtscp(&mut aux);

                *thread_data.combiner_time_stat.get() += end - begin;
            }

            return myNode.data.get().read().assume_init();
        }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        unsafe {
            self.local_node
                .get()
                .map(|node| *node.combiner_time_stat.get())
        }
    }
}
