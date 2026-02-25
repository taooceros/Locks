use crossbeam::queue::ArrayQueue;

use std::{
    cell::SyncUnsafeCell,
    collections::LinkedList,
    ptr::null,
    sync::atomic::{Ordering::*, *},
};
use thread_local::ThreadLocal;

use crate::parker::Parker;

use super::rcllock::RclLockPtr;

use super::{rclrequest::*, rclthread::RclThread};

#[derive(Debug)]
pub struct RclServer<P: Parker + 'static> {
    threads: LinkedList<SyncUnsafeCell<RclThread<P>>>,
    pub(super) prepared_threads: ArrayQueue<*const RclThread<P>>,
    pub(super) num_free_threads: AtomicI32,
    pub(super) num_serving_threads: AtomicI32,
    pub num_clients: AtomicUsize,
    pub(super) timestmap: i32,
    pub(super) is_alive: bool,
    _cpu: usize,
    pub(super) client_id: ThreadLocal<usize>,
    pub(super) requests: Vec<RclRequest<u8, P>>,
}

unsafe impl<P: Parker> Send for RclServer<P> {}
unsafe impl<P: Parker> Sync for RclServer<P> {}

impl<P: Parker> Drop for RclServer<P> {
    fn drop(&mut self) {
        self.is_alive = false;
        for thread in self.threads.iter_mut() {
            thread.get_mut().waiting_to_serve.wake(1);
        }

        for thread in self.threads.iter_mut() {
            thread
                .get_mut()
                .thread_handle
                .take()
                .unwrap()
                .join()
                .unwrap();
        }
    }
}

impl<P: Parker> RclServer<P> {
    pub fn new() -> RclServer<P> {
        RclServer {
            threads: LinkedList::new(),
            prepared_threads: ArrayQueue::new(128),
            num_free_threads: AtomicI32::new(0),
            num_serving_threads: AtomicI32::new(0),
            num_clients: AtomicUsize::new(0),
            timestmap: 0,
            is_alive: true,
            _cpu: 0,
            client_id: ThreadLocal::new(),
            requests: {
                // assert_eq!(size_of::<RclRequestSized>(), size_of::<RclRequest<u8>>());

                // this is very unsafe (bypass even type check) and require careful check
                // the RclRequest should only contains pointer/ref to the data, so size of RclRequest
                // should be the same as size of all types
                let mut v = Vec::with_capacity(128);
                v.resize_with(128, || RclRequest {
                    real_me: 0,
                    lock: RclLockPtr::from(null()),
                    parker: Default::default(),
                    f: SyncUnsafeCell::new(None).into(),
                });

                v
                // panic if unable to allocate
            },
        }
    }

    pub fn start(&mut self, cpuid: usize) {
        self._cpu = cpuid;

        let server_ptr = self as *mut RclServer<P>;

        self.threads.push_back(SyncUnsafeCell::new(RclThread::new(
            server_ptr.into(),
            cpuid,
        )));

        let thread = self.threads.back_mut().unwrap();
        RclThread::run(thread, cpuid);
        self.num_free_threads.fetch_add(1, SeqCst);
    }

    fn start_thread(thread: *mut RclThread<P>) {
        let thread = unsafe { &mut *thread };
        thread.waiting_to_serve.wake(1);
    }
}
