use crate::{atomic_extension::AtomicExtension, dlock2::DLock2};
use std::{
    arch::x86_64::__rdtscp,
    cell::{SyncUnsafeCell, UnsafeCell},
    hint::spin_loop,
    mem::MaybeUninit,
    ptr::null_mut,
    sync::atomic::{AtomicPtr, AtomicU8, Ordering::*},
};

use debug_unwraps::DebugUnwrapExt;
use thread_local::ThreadLocal;

use super::node::Node;
use crate::dlock2::DLock2Delegate;

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
            let my_node = &(*thread_data.nodes.get())[(1 - toggled) as usize];

            my_node.wait.store_release(true);
            my_node.completed.store_release(false);
            my_node.next.store_release(null_mut());

            // announce the request
            my_node.data.get().write(MaybeUninit::new(data));

            // insert the node into the queue
            let my_pred_node = self.tail.swap(my_node.as_mut_ptr(), AcqRel);

            // if a node already exists in the list
            if !my_pred_node.is_null() {
                let pred_node = my_pred_node.as_mut().unwrap_unchecked();

                pred_node.next.store_release(my_node.as_mut_ptr());

                while my_node.wait.load_acquire() {
                    spin_loop();
                }

                if my_node.completed.load_acquire() {
                    return my_node.data.get().read().assume_init();
                }
            }

            // combiner

            #[cfg(feature = "combiner_stat")]
            let begin = __rdtscp(&mut aux);

            let mut tmp_node = my_node;

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
                    // The completed should always be true and wait should always be false because no other node is available in the list
                    // It is not sure whether the acquire ordering is required because the current thread
                    // should be the combiner which means it should handle its own node.
                    if my_node.completed.load_acquire() {
                        return my_node.data.get().read().assume_init();
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

            my_node.data.get().read().assume_init()
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
