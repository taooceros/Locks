use std::{
    alloc::{alloc_zeroed, Layout},
    array,
    collections::LinkedList,
    mem::{size_of, transmute},
    ptr::Unique,
    sync::{
        atomic::{Ordering::*, *},
        *,
    },
};

use lockfree::{stack::Stack, tls::ThreadLocal};

use super::{rclrequest::*, rclthread::RclThread};

pub struct RclServer {
    threads: LinkedList<RclThread>,
    pub(super) prepared_threads: Stack<&'static RclThread>,
    pub(super) num_free_threads: AtomicI32,
    pub(super) num_serving_threads: AtomicI32,
    pub num_clients: AtomicUsize,
    pub(super) timestmap: i32,
    pub(super) is_alive: bool,
    cpu: usize,
    pub(super) client_id: ThreadLocal<usize>,
    pub(super) requests: Vec<RclRequestSized>,
}

impl RclServer {
    pub fn new(cpuid: usize) -> RclServer {
        let mut server = RclServer {
            threads: LinkedList::new(),
            prepared_threads: Stack::new(),
            num_free_threads: AtomicI32::new(0),
            num_clients: AtomicUsize::new(0),
            num_serving_threads: AtomicI32::new(0),
            timestmap: 0,
            is_alive: true,
            cpu: cpuid,
            client_id: ThreadLocal::new(),
            requests: unsafe {
                assert!(size_of::<RclRequestSized>() == size_of::<RclRequest<u8>>());

                // this is very unsafe (bypass even type check) and require careful check
                // the RclRequest should only contains pointer/ref to the data, so size of RclRequest
                // should be the same as size of all types
                let mut v = Vec::with_capacity(128);
                v.resize_with(128, Default::default);
                v
                // panic if unable to allocate
            },
        };

        let thread = RclThread::new(Unique::new(&mut server).unwrap());

        server.threads.push_back(thread);

        return server;
    }

    pub fn register() {}
}
