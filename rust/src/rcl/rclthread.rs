use std::{
    cmp::min,
    mem::transmute,
    sync::atomic::{Ordering::*},
    thread::{self, yield_now, JoinHandle},
};

use linux_futex::{Futex, Private};

use super::{
    rclrequest::{RclRequest, RequestCallable},
    rclserver::*,
};

use crate::syncptr::*;

pub struct RclThread {
    server: SyncMutPtr<RclServer>,
    timestamp: i32,
    pub(super) waiting_to_serve: Futex<Private>,
    pub thread_handle: Option<JoinHandle<()>>,
}

impl RclThread {
    pub fn new(server: SyncMutPtr<RclServer>, _cpuid: usize) -> RclThread {
        let mut thread = RclThread {
            server,
            timestamp: 0,
            waiting_to_serve: Futex::new(0),
            thread_handle: None,
        };

        let _threadptr = SyncMutPtr::from(&mut thread);

        return thread;
    }

    pub fn run(&mut self, cpuid: usize) {
        let server = self.server;
        let threadptr = {
            let ptr: *mut RclThread = self;
            SyncMutPtr::from(ptr)
        };

        self.thread_handle = Some(thread::spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: cpuid });

            let server = server;
            let threadptr = threadptr;

            let server = unsafe { &mut *server.ptr };

            loop {
                // Should change in the future
                if server.is_alive == false {
                    break;
                }
                let thread = unsafe { &mut *threadptr.ptr };

                thread.timestamp = server.timestmap;
                server.num_free_threads.fetch_add(-1, SeqCst);

                let serving_client = server.num_clients.load(Relaxed);

                let length = server.requests.len();

                // println!("{} {}", serving_client, length);

                for req in server.requests.iter_mut().take(min(length, serving_client)) {
                    let req: &mut RclRequest<u8> = unsafe { transmute(req) };

                    // println!("{:?}", req);

                    if req.f.is_none() {
                        continue;
                    }

                    if req.lock.lock.is_null() {
                        continue;
                    }
                    match (*req.lock)
                        .holder
                        .compare_exchange(!0, req.real_me, Relaxed, Relaxed)
                    {
                        Ok(_) => {
                            req.call();
                            (*req.lock).holder.store(!0, Relaxed);
                        }
                        Err(_) => {
                            eprintln!("should not happen");
                        }
                    }
                }
                let free_thread = server.num_free_threads.fetch_add(1, SeqCst);

                if server.num_serving_threads.load(Relaxed) > 1 {
                    if free_thread <= 1 {
                        // yield to other serving threads
                        yield_now();
                    } else {
                        // stop current thread
                        server.num_serving_threads.fetch_add(-1, SeqCst);
                        server.prepared_threads.push(thread);
                        _ = thread.waiting_to_serve.wait(0);
                    }
                }
            }
        }));
    }
}
