#![feature(sync_unsafe_cell)]

use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::thread;

use flatcombining::FcLock;

pub mod flatcombining;

// I have some magic semantics for some synchronization primitive!
#[derive(Debug, Clone, Copy)]
pub struct I32Unsafe(*mut i32);

unsafe impl Send for I32Unsafe {}
unsafe impl Sync for I32Unsafe {}

fn main() {
    let counter = Arc::new(FcLock::new(0));

    let mut handles = vec![];

    let counter_mutex = Arc::new(Mutex::new(0));

    for i in 0..16 {
        let lock_ref = counter.clone();
        let lock_ref_mutex = counter_mutex.clone();

        let handle = thread::Builder::new().name(i.to_string()).spawn(move || {
            println!("Thread {} started", i);
            for _ in 0..100 {
                lock_ref.lock(|mut guard| {
                    for _ in 0..100000 {
                        // unsafe {
                        //     *(counter_ref.0) += 1;
                        // }
                        *guard += 1;

                        // let mut l = lock_ref_mutex.lock().unwrap();
                        // (*l) += 1;
                    }
                });

                // let mut counter = lock_ref_mutex.lock().unwrap();
                // for _ in 0..100000 {
                //     (*counter) += 1;
                // }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.unwrap().join().unwrap();
    }

    counter.lock(|guard| {
        let counter = *guard;
        println!("{}", counter);
    });

    println!("{}", counter_mutex.lock().unwrap());
}
