use core_affinity::*;
use criterion::measurement::WallTime;
use criterion::BenchmarkGroup;
use criterion::BenchmarkId;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dlock::guard::DLockGuard;
use dlock::parker::block_parker::BlockParker;
use dlock::parker::spin_parker::SpinParker;
use dlock::parker::Parker;
use quanta::Clock;
use std::{sync::Arc, thread::*, time::Duration};

extern crate dlock;

use dlock::ccsynch::*;
use dlock::dlock::*;
use dlock::fc::fclock::*;
use dlock::rcl::rcllock::*;
use dlock::rcl::rclserver::*;

const ITERATION: u64 = 1000;
const THREAD_CPU_RATIO: usize = 1;

pub fn lock_bench(bencher: &mut Criterion) {
    let cpu_count = available_parallelism().unwrap().get();

    let mut group = bencher.benchmark_group("Delegation Locks");

    for i in [2, 4, 8].iter() {
        let thread = i * THREAD_CPU_RATIO;
        ccbench::<SpinParker>(&mut group, cpu_count, thread);
        fcbench::<SpinParker>(&mut group, cpu_count, thread);
        rclbench::<SpinParker>(&mut group, cpu_count, thread);
        ccbench::<BlockParker>(&mut group, cpu_count, thread);
        fcbench::<BlockParker>(&mut group, cpu_count, thread);
        rclbench::<BlockParker>(&mut group, cpu_count, thread);
    }

    group.finish();
}

pub fn ccbench<P: Parker + 'static>(
    bencher: &mut BenchmarkGroup<WallTime>,
    cpu_count: usize,
    thread_count: usize,
) {
    let lock = Arc::new(DLockType::<u64, P>::CCSynch(CCSynch::new(0u64)));

    bench_inner(lock.clone(), "ccsynch", bencher, cpu_count, thread_count)
}

pub fn fcbench<P: Parker + 'static>(
    bencher: &mut BenchmarkGroup<WallTime>,
    cpu_count: usize,
    thread_count: usize,
) {
    let lock = Arc::new(DLockType::<u64, P>::FlatCombining(FcLock::new(0u64)));

    bench_inner(
        lock.clone(),
        "flat combining",
        bencher,
        cpu_count,
        thread_count,
    )
}

pub fn rclbench<P: Parker + 'static>(
    bencher: &mut BenchmarkGroup<WallTime>,
    cpu_count: usize,
    thread_count: usize,
) {
    let mut server = RclServer::new();

    server.start(cpu_count - 1);

    let cc = Arc::new(DLockType::<u64, P>::RCL(RclLock::new(&mut server, 0u64)));

    bench_inner(
        cc.clone(),
        "remote-core-locking",
        bencher,
        cpu_count - 1,
        thread_count,
    )
}

#[inline]
fn bench_inner<P: Parker + 'static>(
    lock: Arc<DLockType<u64, P>>,
    name: &str,
    bencher: &mut BenchmarkGroup<WallTime>,
    cpu_count: usize,
    thread_count: usize,
) {
    bencher.bench_with_input(BenchmarkId::new(name, thread_count), &cpu_count, |b, _i| {
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

fn cooperative_counter<P: Parker + 'static>(
    lock: Arc<DLockType<u64, P>>,
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
                    let _single_iter_duration =
                        Duration::from_micros(if id % 2 == 0 { 1 } else { 3 });
                    let timer = Clock::new();
                    let mut now_value = 0;
                    while now_value < threshold {
                        // println!("{}", now_value);
                        lock.lock(&mut |mut guard: DLockGuard<u64>| {
                            let _begin = timer.now();
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
