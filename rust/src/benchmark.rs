
use serde_with::DurationMilliSeconds;

use std::num::NonZeroI64;
use std::path::Path;
use std::{
    fs::File,
    sync::{atomic::*, Arc},
    thread::{self, JoinHandle},
    time::Duration,
};


use csv::Writer;
use itertools::Itertools;
use libdlock::{
    dlock::{BenchmarkType, DLock, DLockType},
    guard::DLockGuard,
    parker::{block_parker::BlockParker, spin_parker::SpinParker, Parker},
    rcl::{rcllock::RclLock, rclserver::RclServer},
};

use serde::Serialize;
use serde_with::serde_as;
use strum::IntoEnumIterator;

use crate::benchmark::counter_job::one_three_benchmark;
use crate::benchmark::subversion_job::subversion_benchmark;
use crate::command_parser::*;

mod counter_job;
mod subversion_job;

pub fn benchmark(
    num_cpu: usize,
    num_thread: usize,
    experiment: Option<Experiment>,
    target: Option<LockTarget>,
    output_path: &Path,
    waiter: WaiterType,
    duration: u64,
) {
    let experiments = match experiment {
        Some(e) => vec![e],
        None => experiment.into_iter().collect(),
    };

    for experiment in experiments {
        let writer = &mut Writer::from_path(output_path.join("output.csv")).unwrap();

        let job = match experiment {
            Experiment::RatioOneThree => one_three_benchmark,
            Experiment::Subversion => subversion_benchmark,
        };

        let targets = extract_targets(waiter, target);

        for target in targets {
            if let Some(lock) = target {
                inner_benchmark(Arc::new(lock), num_cpu, num_thread, writer, duration, job);
            }
        }

        if matches!(target, Some(LockTarget::DLock(DLockTarget::RCL)) | None) {
            match waiter {
                WaiterType::Spin => {
                    bench_rcl::<_, _, SpinParker>(num_cpu, num_thread, writer, duration, job)
                }
                WaiterType::Block => {
                    bench_rcl::<_, _, BlockParker>(num_cpu, num_thread, writer, duration, job)
                }
                WaiterType::All => {
                    bench_rcl::<_, _, SpinParker>(num_cpu, num_thread, writer, duration, job);
                    bench_rcl::<_, _, BlockParker>(num_cpu, num_thread, writer, duration, job)
                }
            }
        }
        println!("{:?} finished", experiment);
    }
}

fn extract_targets(
    waiter: WaiterType,
    target: Option<LockTarget>,
) -> Vec<Option<BenchmarkType<u64>>> {
    let targets: Vec<Option<BenchmarkType<u64>>> = match waiter {
        WaiterType::Spin => match target {
            Some(target) => vec![target.to_locktype::<SpinParker>()],
            None => (LockTarget::iter().map(|t| t.to_locktype::<SpinParker>())).collect(),
        },
        WaiterType::Block => match target {
            Some(target) => vec![target.to_locktype::<BlockParker>()],
            None => (LockTarget::iter().map(|t| t.to_locktype::<BlockParker>())).collect(),
        },
        WaiterType::All => match target {
            Some(target) => {
                let mut locks = vec![target.to_locktype::<SpinParker>()];
                if matches!(target, LockTarget::DLock(_)) {
                    locks.push(target.to_locktype::<BlockParker>());
                }
                locks
            }
            None => (LockTarget::iter().map(|t| t.to_locktype::<SpinParker>()))
                .interleave(
                    LockTarget::iter()
                        .filter(|x| matches!(x, LockTarget::DLock(_)))
                        .map(|t| t.to_locktype::<BlockParker>()),
                )
                .collect(),
        },
    };
    targets
}

fn bench_rcl<T: Send + Sync + 'static, R: Serialize, P>(
    num_cpu: usize,
    num_thread: usize,
    writer: &mut Writer<File>,
    duration: u64,
    job: impl FnOnce(Arc<BenchmarkType<T>>, usize, usize, usize, &'static AtomicBool) -> R
        + Send
        + 'static,
) where
    BenchmarkType<u64>: From<DLockType<u64, P>>,
    P: Parker + 'static,
{
    let mut server = RclServer::new();
    server.start(num_cpu - 1);
    let lock = RclLock::new(&mut server, 0u64);

    inner_benchmark(
        Arc::new(DLockType::<u64, P>::RCL(lock).into()),
        num_cpu - 1,
        num_thread,
        writer,
        duration,
        job,
    );
}

fn inner_benchmark<T: Send + Sync + 'static, R: Serialize>(
    lock_type: Arc<BenchmarkType<u64>>,
    num_cpu: usize,
    num_thread: usize,
    writer: &mut Writer<File>,
    duration: u64,
    _job: impl FnOnce(Arc<BenchmarkType<T>>, usize, usize, usize, &'static AtomicBool) -> R
        + Send
        + 'static,
) where
    R: Serialize,
{
    static STOP: AtomicBool = AtomicBool::new(false);

    STOP.store(false, Ordering::Release);

    let threads = (0..num_thread)
        .map(|id| {
            benchmark_num_threads(
                lock_type.clone(),
                id,
                num_thread,
                num_cpu,
                &STOP,
                one_three_benchmark,
            )
        })
        .collect::<Vec<_>>();

    println!("Starting benchmark for {}", lock_type);

    let mut results = Vec::new();

    thread::sleep(Duration::from_secs(duration as u64));

    STOP.store(true, Ordering::Release);

    let mut i = 0;

    for thread in threads {
        let l = thread.join();
        match l {
            Ok(l) => {
                results.push(l);
                // println!("{}", l);
            }
            Err(_e) => eprintln!("Error joining thread: {}", i),
        }
        i += 1;
    }

    for result in results.iter() {
        writer.serialize(result).unwrap();
    }

    let total_count: u64 = results.iter().map(|r| r.loop_count).sum();

    lock_type.lock(|guard: DLockGuard<u64>| {
        assert_eq!(
            *guard, total_count,
            "Total counter is not matched with lock value {}, but thread local loop sum {}",
            *guard, total_count
        );
    });

    println!(
        "Finish Benchmark for {}: Total Counter {}",
        lock_type, total_count
    );
}

#[serde_as]
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Record {
    id: usize,
    cpu_id: usize,
    thread_num: usize,
    cpu_num: usize,
    loop_count: u64,
    num_acquire: u64,
    #[serde_as(as = "DurationMilliSeconds<u64>")]
    hold_time: Duration,
    #[cfg(feature = "combiner_stat")]
    combine_time: Option<NonZeroI64>,
    locktype: String,
    waiter_type: String,
}

fn benchmark_num_threads<T: Send + Sync, R: Serialize + Send + Sync + 'static>(
    lock_type: Arc<BenchmarkType<T>>,
    id: usize,
    num_thread: usize,
    num_cpu: usize,
    stop: &'static AtomicBool,
    job: impl FnOnce(Arc<BenchmarkType<T>>, usize, usize, usize, &'static AtomicBool) -> R
        + Send
        + 'static,
) -> JoinHandle<R> {
    let lock_type = lock_type.clone();

    thread::Builder::new()
        .name(id.to_string())
        .spawn(move || job(lock_type, id, num_thread, num_cpu, stop))
        .unwrap()
}
