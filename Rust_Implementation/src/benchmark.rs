use crate::{
    ccsynch::CCSynch,
    dlock::*,
    flatcombining::FcLock,
    rcl::{rcllock::RclLock, rclserver::RclServer},
};
extern crate test;
use core_affinity::*;
use quanta::Clock;
use std::{sync::Arc, thread::*, time::Duration};
use test::Bencher;

const ITERATION: u64 = 100000000000;

#[bench]
pub fn ccbench(bencher: &mut Bencher) {
    let cpu_count = available_parallelism().unwrap();
    let thread_count = cpu_count;

    bencher.iter(|| {
        let cc = Arc::new(LockType::CCSynch(CCSynch::new(0u64)));
        cooperative_counter((cc), cpu_count.get(), thread_count.get(), ITERATION);
    });
}

#[bench]
pub fn fcbench(bencher: &mut Bencher) {
    let cpu_count = available_parallelism().unwrap();
    let thread_count = cpu_count;

    bencher.iter(|| {
        let cc = Arc::new(LockType::FlatCombining(FcLock::new(0u64)));
        cooperative_counter((cc), cpu_count.get(), thread_count.get(), ITERATION);
    });
}

#[bench]
pub fn rclbench(bencher: &mut Bencher) {
    let cpu_count = available_parallelism().unwrap();
    let thread_count = cpu_count;

    let mut server = RclServer::new(cpu_count.get() - 1);

    let server_ptr = &mut server as *mut RclServer;
    bencher.iter(|| {
        let cc = Arc::new(LockType::RCL(RclLock::new(server_ptr, 0u64)));
        cooperative_counter((cc), cpu_count.get() - 1, thread_count.get(), ITERATION);
    });
}

fn cooperative_counter(
    lock: Arc<LockType<u64>>,
    cpu_count: usize,
    thread_count: usize,
    threshold: u64,
) {
    (1..thread_count).map(|id| {
        let cpu_count = cpu_count.clone();
        let id = id;
        let lock = lock.clone();
        Builder::new().name(id.to_string()).spawn(move || {
            set_for_current(CoreId {
                id: (id % cpu_count),
            });
            let single_iter_duration = Duration::from_micros(if id % 2 == 0 { 100 } else { 300 });
            let timer = Clock::new();
            let mut now_value = 0;
            while (now_value < threshold) {
                lock.lock(&mut |guard| {
                    let begin = timer.now();
                    loop {
                        now_value = **guard;
                        if now_value >= threshold {
                            break;
                        }
                        if timer.now().duration_since(begin) >= single_iter_duration {
                            break;
                        }

                        **guard += 1;
                    }
                })
            }
        })
    });
    return;
}
