use std::{
    cell::SyncUnsafeCell,
    cmp::min,
    mem::transmute,
    ptr::Unique,
    sync::atomic::{AtomicBool, Ordering::*},
    thread::{self, yield_now, JoinHandle},
};

use linux_futex::{Futex, Private};

use super::{
    rclrequest::{RclRequest, RequestCallable},
    rclserver::*,
};

pub struct RclThread {
    server: Unique<RclServer>,
    timestamp: i32,
    waiting_to_serve: Futex<Private>,
    thread_handle: Option<JoinHandle<()>>,
}

impl RclThread {
    pub fn new(mut server: Unique<RclServer>) -> RclThread {
        let mut thread = RclThread {
            server,
            timestamp: 0,
            waiting_to_serve: Futex::new(0),
            thread_handle: None,
        };

        let threadptr = Unique::new(&mut thread).unwrap();

        thread.thread_handle = Some(thread::spawn(move || unsafe {
            let server = server.as_mut();
            loop {
                server.is_alive = true;
                let thread = &mut *threadptr.as_ptr();
                thread.timestamp = server.timestmap;
                server.num_free_threads.fetch_add(-1, SeqCst);

                let serving_client = server.num_clients.load(Relaxed);

                let length = server.requests.len();
                for req in server.requests.iter_mut().take(min(length, serving_client)) {
                    let req: &mut RclRequest<u8> = transmute(req);
                    match (*req.lock).holder.compare_exchange(
                        0,
                        req.real_me + 1,
                        Relaxed,
                        Relaxed,
                    ) {
                        Ok(_) => {
                            req.call();
                            (*req.lock).holder.store(0, Relaxed);
                        }
                        Err(_) => {}
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
                        server.prepared_threads.push(transmute(&thread));
                        _ = thread.waiting_to_serve.wait(0);
                    }
                }
            }
        }));

        return thread;
    }

    pub fn run() {}
}
