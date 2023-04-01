use std::{
    collections::LinkedList,
    mem::{size_of},
    ptr::{null},
    sync::{
        atomic::{Ordering::*, *},
    },
};

use lockfree::{stack::Stack, tls::ThreadLocal};

use super::rcllock::RclLockPtr;

use super::{rclrequest::*, rclthread::RclThread};

pub struct RclServer {
    threads: LinkedList<RclThread>,
    pub(super) prepared_threads: Stack<&'static RclThread>,
    pub(super) num_free_threads: AtomicI32,
    pub(super) num_serving_threads: AtomicI32,
    pub num_clients: AtomicUsize,
    pub(super) timestmap: i32,
    pub(super) is_alive: bool,
    _cpu: usize,
    pub(super) client_id: ThreadLocal<usize>,
    pub(super) requests: Vec<RclRequest<u8>>,
}

impl Drop for RclServer {
    fn drop(&mut self) {
        self.is_alive = false;
        for thread in self.threads.iter_mut() {
            thread.waiting_to_serve.wake(1);
        }

        for thread in self.threads.iter_mut() {
            thread.thread_handle.take().unwrap().join().unwrap();
        }
    }
}

impl RclServer {
    pub fn new(cpuid: usize) -> RclServer {
        let mut server = RclServer {
            threads: LinkedList::new(),
            prepared_threads: Stack::new(),
            num_free_threads: AtomicI32::new(0),
            num_serving_threads: AtomicI32::new(0),
            num_clients: AtomicUsize::new(0),
            timestmap: 0,
            is_alive: true,
            _cpu: cpuid,
            client_id: ThreadLocal::new(),
            requests: {
                assert!(size_of::<RclRequestSized>() == size_of::<RclRequest<u8>>());

                // this is very unsafe (bypass even type check) and require careful check
                // the RclRequest should only contains pointer/ref to the data, so size of RclRequest
                // should be the same as size of all types
                let mut v = Vec::with_capacity(128);
                v.resize_with(128, || RclRequest {
                    real_me: 0,
                    lock: RclLockPtr::from(null()),
                    f: None,
                });

                v
                // panic if unable to allocate
            },
        };

        let thread = RclThread::new((&mut server).into(), cpuid);

        server.threads.push_back(thread);

        let thread = server.threads.back_mut().unwrap();
        thread.run(cpuid);
        server.num_free_threads.fetch_add(1, SeqCst);
        return server;
    }

    fn start_thread(thread : *mut RclThread) {
        let thread = unsafe { &mut *thread };
        thread.waiting_to_serve.wake(1);
    }
}
