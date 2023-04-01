use std::{
    sync::{Arc, Mutex},
    thread,
};

use crate::{
    ccsynch::CCSynch,
    dlock::{DLock, LockType},
    flatcombining::FcLock,
    rcl::{rcllock::RclLock, rclserver::RclServer},
};

#[test]
pub fn test_lock() {
    let fc_lock = Arc::new(LockType::FlatCombining(FcLock::new(0usize)));
    let cc_lock = Arc::new(LockType::CCSynch(CCSynch::new(0usize)));
    let mut server = RclServer::new(15);
    let server_ptr: *mut RclServer = &mut server;
    let rcl_lock = Arc::new(LockType::RCL(RclLock::new(server_ptr, 0)));
    inner_test(fc_lock);
    inner_test(cc_lock);
    inner_test(rcl_lock);
}

const THREAD_NUM: usize = 12;
const ITERATION: usize = 100000;
const INNER_ITERATION: usize = 100000;

pub fn inner_test(lock: Arc<LockType<usize>>) {
    let mut handles = vec![];

    let counter_mutex = Arc::new(Mutex::new(0i64));

    for i in 0..THREAD_NUM {
        let lock_ref = lock.clone();
        let _lock_ref_mutex = counter_mutex.clone();

        let handle = thread::Builder::new().name(i.to_string()).spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: i as usize });
            for _ in 0..ITERATION {
                lock_ref.lock(&mut |guard| {
                    for _ in 0..INNER_ITERATION {
                        **guard += 1;
                    }
                });
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.unwrap().join().unwrap();
    }

    lock.lock(&mut |guard| assert!(**guard == (THREAD_NUM * ITERATION * INNER_ITERATION)));

    println!("finish testing {}", lock);
}
