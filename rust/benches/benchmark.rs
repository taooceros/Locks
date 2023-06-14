use core_affinity::*;
use criterion::measurement::WallTime;
use criterion::BenchmarkGroup;
use criterion::BenchmarkId;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dlock::guard::DLockGuard;
use quanta::Clock;
use std::{sync::Arc, thread::*, time::Duration};

extern crate dlock;

use dlock::ccsynch::*;
use dlock::dlock::*;
use dlock::flatcombining::fclock::*;
use dlock::rcl::rcllock::*;
use dlock::rcl::rclserver::*;

const ITERATION: u64 = 1000;
const THREAD_CPU_RATIO: usize = 1;

pub fn lock_bench(bencher: &mut Criterion) {
    let cpu_count = available_parallelism().unwrap().get();

    let mut group = bencher.benchmark_group("Delegation Locks");

    for i in [2, 4, 8].iter() {
        let thread = i * THREAD_CPU_RATIO;
        ccbench(&mut group, cpu_count, thread);
        fcbench(&mut group, cpu_count, thread);
        rclbench(&mut group, cpu_count, thread);
    }

    group.finish();
}

pub fn ccbench(bencher: &mut BenchmarkGroup<WallTime>, cpu_count: usize, thread_count: usize) {
    let lock = Arc::new(LockType::CCSynch(CCSynch::new(0u64)));

    bench_inner(lock.clone(), "ccsynch", bencher, cpu_count, thread_count)
}

pub fn fcbench(bencher: &mut BenchmarkGroup<WallTime>, cpu_count: usize, thread_count: usize) {
    let lock = Arc::new(LockType::FlatCombining(FcLock::new(0u64)));

    bench_inner(
        lock.clone(),
        "flat combining",
        bencher,
        cpu_count,
        thread_count,
    )
}

pub fn rclbench(bencher: &mut BenchmarkGroup<WallTime>, cpu_count: usize, thread_count: usize) {
    let mut server = RclServer::new();

    server.start(cpu_count - 1);

    let server_ptr = &mut server as *mut RclServer;
    let cc = Arc::new(LockType::RCL(RclLock::new(server_ptr, 0u64)));

    bench_inner(
        cc.clone(),
        "remote-core-locking",
        bencher,
        cpu_count - 1,
        thread_count,
    )
}

#[inline]
fn bench_inner(
    lock: Arc<LockType<u64>>,
    name: &str,
    bencher: &mut BenchmarkGroup<WallTime>,
    cpu_count: usize,
    thread_count: usize,
) {
    bencher.bench_with_input(BenchmarkId::new(name, thread_count), &cpu_count, |b, i| {
        b.iter(|| {
            let lock = lock.clone();
            lock.lock(&mut |mut guard: DLockGuard<u64>| {
                *guard = 0;
            });
            black_box(cooperative_counter(
                lock.clone(),
                cpu_count - 1,
                thread_count,
                ITERATION,
            ));

            lock.lock(&mut |guard: DLockGuard<u64>| {
                assert_eq!(ITERATION, *guard);
            });
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
                    while now_value < threshold {
                        // println!("{}", now_value);
                        lock.lock(&mut |mut guard: DLockGuard<u64>| {
                            let begin = timer.now();
                            now_value = *guard;
                            if now_value >= threshold {
                                return;
                            }

                            *guard += 1;
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
