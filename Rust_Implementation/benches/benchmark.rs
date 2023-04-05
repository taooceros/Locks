use core_affinity::*;
use quanta::Clock;
use std::{sync::Arc, thread::*, time::Duration};
use tester::rcl::rcllock::*;
use tester::rcl::rclserver::*;

extern crate tester;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tester::ccsynch::*;
use tester::dlock::*;
use tester::flatcombining::*;
use tester::rcl::*;

const ITERATION: u64 = 100000;
const THREAD_CPU_RATIO: usize = 1;

pub fn ccbench(bencher: &mut Criterion) {
    let cpu_count = available_parallelism().unwrap();
    let thread_count = cpu_count.get() * THREAD_CPU_RATIO;

    bencher.bench_function("cc synch benchmark", |b| {
        b.iter(|| {
            let cc = Arc::new(LockType::CCSynch(CCSynch::new(0u64)));
            cooperative_counter((cc), cpu_count.get(), thread_count, ITERATION);
        });
    });
}

pub fn fcbench(bencher: &mut Criterion) {
    let cpu_count = available_parallelism().unwrap();
    let thread_count = cpu_count.get() * THREAD_CPU_RATIO;

    bencher.bench_function("flatcombining benchmark", |b| {
        b.iter(|| {
            let cc = Arc::new(LockType::FlatCombining(FcLock::new(0u64)));
            cooperative_counter((cc), cpu_count.get(), thread_count, ITERATION);
        });
    });
}

pub fn rclbench(bencher: &mut Criterion) {
    let cpu_count = available_parallelism().unwrap();
    let thread_count = cpu_count.get() * THREAD_CPU_RATIO;

    let mut server = RclServer::new(cpu_count.get() - 1);

    let server_ptr = &mut server as *mut RclServer;
    bencher.bench_function("remote core locking benchmark", |b| {
        b.iter(|| {
            let cc = Arc::new(LockType::RCL(RclLock::new(server_ptr, 0u64)));
            cooperative_counter((cc), cpu_count.get() - 1, thread_count, ITERATION);
        });
    });
}

fn cooperative_counter(
    lock: Arc<LockType<u64>>,
    cpu_count: usize,
    thread_count: usize,
    threshold: u64,
) {
    let res = (1..thread_count)
        .map(|id| {
            let num_cpusu_count = cpu_count.clone();
            let id = id;
            let lock = lock.clone();
            Builder::new()
                .name(id.to_string())
                .spawn(move || {
                    set_for_current(CoreId {
                        id: (id % cpu_count),
                    });
                    let single_iter_duration =
                        Duration::from_micros(if id % 2 == 0 { 1 } else { 3 });
                    let timer = Clock::new();
                    let mut now_value = 0;
                    while (now_value < threshold) {
                        // println!("{}", now_value);
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
                .unwrap()
        })
        .collect::<Vec<_>>();

    for thread in res {
        thread.join().unwrap();
    }
    return;
}

criterion_group!(benches, ccbench, fcbench, rclbench);

criterion_main!(benches);
