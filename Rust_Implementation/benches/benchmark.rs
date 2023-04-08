use core_affinity::*;
use criterion::measurement::WallTime;
use criterion::BenchmarkGroup;
use criterion::BenchmarkId;
use quanta::Clock;
use std::fmt;
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

pub fn lock_bench(bencher: &mut Criterion) {
    let cpu_count = available_parallelism().unwrap().get();
    let thread_count = cpu_count * THREAD_CPU_RATIO;

    let mut group = bencher.benchmark_group("Delegation Locks");

    for i in [2, 4, 8, 16, 32, 64, 128].iter() {
        let thread = i * THREAD_CPU_RATIO;
        ccbench(&mut group, cpu_count, thread);
        fcbench(&mut group, cpu_count, thread);
        rclbench(&mut group, cpu_count, thread);
    }

    group.finish();
}

pub fn ccbench(bencher: &mut BenchmarkGroup<WallTime>, cpu_count: usize, thread_count: usize) {
    bencher.bench_with_input(
        BenchmarkId::new("cc-synch", thread_count),
        &cpu_count,
        |b, i| {
            b.iter(|| {
                let cc = Arc::new(LockType::CCSynch(CCSynch::new(0u64)));
                cooperative_counter(cc, cpu_count, thread_count, ITERATION);
            });
        },
    );
}

pub fn fcbench(bencher: &mut BenchmarkGroup<WallTime>, cpu_count: usize, thread_count: usize) {
    bencher.bench_with_input(
        BenchmarkId::new("flatcombining", &thread_count),
        &cpu_count,
        |b, i| {
            b.iter(|| {
                let cc = Arc::new(LockType::FlatCombining(FcLock::new(0u64)));
                cooperative_counter(cc, cpu_count, thread_count, ITERATION);
            });
        },
    );
}

pub fn rclbench(bencher: &mut BenchmarkGroup<WallTime>, cpu_count: usize, thread_count: usize) {
    let mut server = RclServer::new(cpu_count - 1);

    let server_ptr = &mut server as *mut RclServer;

    bencher.bench_with_input(
        BenchmarkId::new("remote-core-locking", thread_count),
        &cpu_count,
        |b, i| {
            b.iter(|| {
                let cc = Arc::new(LockType::RCL(RclLock::new(server_ptr, 0u64)));
                cooperative_counter(cc, cpu_count - 1, thread_count, ITERATION);
            });
        },
    );
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

criterion_group!(benches, lock_bench);

criterion_main!(benches);
