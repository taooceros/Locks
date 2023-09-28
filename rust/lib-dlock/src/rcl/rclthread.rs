use std::{
    cell::{SyncUnsafeCell},
    cmp::min,
    sync::atomic::{AtomicPtr, Ordering::*},
    thread::{self, yield_now, JoinHandle},
};

use linux_futex::{Futex, Private};

use super::{rclrequest::RequestCallable, rclserver::*};

use crate::{
    parker::{Parker, State},
    syncptr::*,
};

#[derive(Debug)]
pub struct RclThread<P: Parker + 'static> {
    server: AtomicPtr<RclServer<P>>,
    timestamp: i32,
    pub(super) waiting_to_serve: Futex<Private>,
    pub thread_handle: Option<JoinHandle<()>>,
}

unsafe impl<P: Parker> Send for RclThread<P> {}
unsafe impl<P: Parker> Sync for RclThread<P> {}

impl<P: Parker> RclThread<P> {
    pub fn new(server: *mut RclServer<P>, _cpuid: usize) -> RclThread<P> {
        let mut thread = RclThread {
            server: AtomicPtr::new(server),
            timestamp: 0,
            waiting_to_serve: Futex::new(0),
            thread_handle: None,
        };

        let _threadptr = SyncMutPtr::from(&mut thread);

        return thread;
    }

    pub fn run(thread_cell: &SyncUnsafeCell<RclThread<P>>, cpuid: usize) {
        let thread = unsafe { &mut *(thread_cell).get() };
        let server = unsafe { &mut *thread.server.load(Relaxed) };

        let thread2 = unsafe { &mut *thread_cell.get() };

        thread.thread_handle = Some(
            thread::Builder::new()
                .name("server".to_string())
                .spawn(move || {
                    core_affinity::set_for_current(core_affinity::CoreId { id: cpuid });

                    let thread = thread2;

                    loop {
                        // Should change in the future
                        if server.is_alive == false {
                            break;
                        }

                        thread.timestamp = server.timestmap;
                        server.num_free_threads.fetch_add(-1, SeqCst);

                        let serving_client = server.num_clients.load(Relaxed);

                        let length = server.requests.len();

                        // println!("{} {}", serving_client, length);

                        for req in server.requests.iter_mut().take(min(length, serving_client)) {
                            // println!("{:?}", req);

                            // assert!((req.f.get()).is_aligned());

                            let state = req.parker.state();

                            if state != State::Parked {
                                continue;
                            }

                            if req.lock.lock.is_null() {
                                panic!("lock is null");
                            }

                            req.parker.prewake();

                            _ = (*req.lock)
                                .holder
                                .compare_exchange(!0, req.real_me, Relaxed, Relaxed)
                                .expect("should not happen as we only have one thread");

                            req.call();
                            (*req.lock).holder.store(!0, Relaxed);

                            req.parker.wake();
                        }
                        let free_thread = server.num_free_threads.fetch_add(1, SeqCst);

                        if server.num_serving_threads.load(Relaxed) > 1 {
                            if free_thread <= 1 {
                                // yield to other serving threads
                                yield_now();
                            } else {
                                // stop current thread
                                server.num_serving_threads.fetch_add(-1, SeqCst);
                                server.prepared_threads.push(thread).unwrap();
                                _ = thread.waiting_to_serve.wait(0);
                            }
                        }
                    }
                })
                .unwrap(),
        );
    }
}
