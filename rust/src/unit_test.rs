use std::{
    sync::{Arc, Mutex},
    thread::{self, available_parallelism},
};

use serial_test::serial;

use crate::{
    ccsynch::CCSynch,
    dlock::{DLock, LockType},
    flatcombining::FcLock,
    guard::DLockGuard,
    rcl::{rcllock::RclLock, rclserver::RclServer},
};

#[test]
pub fn fc_test() {
    let cpu_count = available_parallelism().unwrap().get();

    let fc_lock = Arc::new(LockType::from(FcLock::new(0usize)));
    inner_test(fc_lock, cpu_count);

    // rcl need one cpu free
}

#[test]
pub fn cc_test() {
    let cpu_count = available_parallelism().unwrap().get();

    let cc_lock = Arc::new(LockType::from(CCSynch::new(0usize)));
    inner_test(cc_lock, cpu_count);
}

#[test]
pub fn rcl_test() {
    let cpu_count = available_parallelism().unwrap().get();
    let mut server = RclServer::new();
    server.start(cpu_count - 1);
    let server_ptr: *mut RclServer = &mut server;
    let rcl_lock = Arc::new(LockType::from(RclLock::new(server_ptr, 0)));
    inner_test(rcl_lock, cpu_count - 1);
}

const THREAD_NUM: usize = 127;
const ITERATION: usize = 10000;
const INNER_ITERATION: usize = 100000;

pub fn inner_test(lock: Arc<LockType<usize>>, cpu_count: usize) {
    let mut handles = vec![];

    let counter_mutex = Arc::new(Mutex::new(0i64));

    for i in 0..THREAD_NUM {
        let lock_ref = lock.clone();
        let _lock_ref_mutex = counter_mutex.clone();

        let handle = thread::Builder::new().name(i.to_string()).spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: i % cpu_count });
            for _ in 0..ITERATION {
                lock_ref.lock(&mut |mut guard: DLockGuard<usize>| {
                    for _ in 0..INNER_ITERATION {
                        *guard += 1;
                    }
                });
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.unwrap().join().unwrap();
    }

    lock.lock(&mut |guard: DLockGuard<usize>| {
        assert!(*guard == (THREAD_NUM * ITERATION * INNER_ITERATION))
    });

    println!("finish testing {}", lock);
}
