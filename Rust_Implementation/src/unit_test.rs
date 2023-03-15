use std::{sync::{Arc, Mutex}, thread};

use crate::{ccsynch::CCSynch};

pub fn test_lock() {
    let counter = Arc::new(CCSynch::new(0i64));

    let mut handles = vec![];

    let counter_mutex = Arc::new(Mutex::new(0i64));

    for i in 0..32 {
        let lock_ref = counter.clone();
        let _lock_ref_mutex = counter_mutex.clone();

        let handle = thread::Builder::new().name(i.to_string()).spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: i as usize });
            // println!("Thread {} started", i);
            for _ in 0..100 {
                lock_ref.lock(&mut |guard| {
                    for _ in 0..1000 {
                        // unsafe {
                        //     *(counter_ref.0) += 1;
                        // }
                        **guard += 1;

                        // let mut l = lock_ref_mutex.lock().unwrap();
                        // (*l) += 1;
                    }
                });

                // let mut counter = _lock_ref_mutex.lock().unwrap();
                // let mut l = || {
                //     for _ in 0..1000000 {
                //         (*counter) += 1;
                //     }
                // };
                // l();
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.unwrap().join().unwrap();
    }

    counter.lock(&mut |guard| {
        println!("Counter: {}", **guard);
    });

    println!("{}", counter_mutex.lock().unwrap());
}
