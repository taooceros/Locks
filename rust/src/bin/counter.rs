use std::{
    fs::{create_dir, remove_dir_all, File},
    num::NonZeroI64,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, available_parallelism, JoinHandle},
    time::Duration,
};

use clap::{Args, Parser, Subcommand};
use csv::Writer;
use quanta::Clock;

use dlock::{
    ccsynch::CCSynch,
    dlock::{DLock, LockType},
    fc_fair_ban::FcFairBanLock,
    fc_fair_ban_slice::FcFairBanSliceLock,
    flatcombining::fclock::FcLock,
    guard::DLockGuard,
    rcl::{rcllock::RclLock, rclserver::RclServer},
};
use dlock::{fc_fair_skiplist::FcSL, spin_lock::SpinLock};

use serde::Serialize;
use serde_with::serde_as;
use serde_with::DurationMilliSeconds;

const DURATION: u64 = 10;

#[serde_as]
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Record<T: 'static> {
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
    locktype: Arc<LockType<T>>,
}

fn benchmark(num_cpu: usize, num_thread: usize, writer: &mut Writer<File>, target: &LockTarget) {
    let locks = match target {
        LockTarget::FcSL => vec![LockType::from(FcSL::new(0u64))],
        LockTarget::FcLock => vec![LockType::from(FcLock::new(0u64))],
        LockTarget::FcFairBanLock => vec![LockType::from(FcFairBanLock::new(0u64))],
        LockTarget::FcFairBanSliceLock => vec![LockType::from(FcFairBanSliceLock::new(0u64))],
        LockTarget::SpinLock => vec![LockType::from(SpinLock::new(0u64))],
        LockTarget::Mutex => vec![LockType::from(Mutex::new(0u64))],
        LockTarget::CCSynch => vec![LockType::from(CCSynch::new(0u64))],
        // need special case for RCL
        LockTarget::RCL => vec![],
        LockTarget::All => vec![
            LockType::from(FcSL::new(0u64)),
            LockType::from(FcLock::new(0u64)),
            LockType::from(FcFairBanLock::new(0u64)),
            LockType::from(FcFairBanSliceLock::new(0u64)),
            LockType::from(SpinLock::new(0u64)),
            LockType::from(Mutex::new(0u64)),
            LockType::from(CCSynch::new(0u64)),
        ],
    };

    for lock in locks {
        inner_benchmark(Arc::new(lock), num_cpu, num_thread, writer);
    }

    if matches!(target, LockTarget::RCL | LockTarget::All) {
        let mut server = RclServer::new();
        server.start(num_cpu - 1);
        let lock = RclLock::new(&mut server, 0u64);
        inner_benchmark(
            Arc::new(LockType::RCL(lock)),
            num_cpu - 1,
            num_thread,
            writer,
        );
    }

    println!("Benchmark finished");
}

fn inner_benchmark(
    lock_type: Arc<LockType<u64>>,
    num_cpu: usize,
    num_thread: usize,
    writer: &mut Writer<File>,
) {
    static STOP: AtomicBool = AtomicBool::new(false);

    STOP.store(false, Ordering::Release);

    let threads = (0..num_thread)
        .map(|id| benchmark_num_threads(&lock_type, id, num_thread, num_cpu, &STOP))
        .collect::<Vec<_>>();

    println!("Starting benchmark for {}", lock_type);

    let mut results = Vec::new();

    thread::sleep(Duration::from_secs(DURATION));

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
        assert_eq!(*guard, total_count);
    });

    println!(
        "Finish Benchmark for {}: Total Counter {}",
        lock_type, total_count
    );
}

fn benchmark_num_threads(
    lock_type_ref: &Arc<LockType<u64>>,
    id: usize,
    num_thread: usize,
    num_cpu: usize,
    stop: &'static AtomicBool,
) -> JoinHandle<Record<u64>> {
    let lock_type = lock_type_ref.clone();

    thread::Builder::new()
        .name(id.to_string())
        .spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: id % num_cpu });
            let single_iter_duration: Duration = Duration::from_micros({
                if id % 2 == 0 {
                    10
                } else {
                    30
                }
            });

            let mut loop_result = 0u64;
            let mut num_acquire = 0u64;
            let mut hold_time = Duration::ZERO;

            while !stop.load(Ordering::Acquire) {
                lock_type.lock(|mut guard: DLockGuard<u64>| {
                    num_acquire += 1;
                    let timer = Clock::new();
                    let begin = timer.now();

                    while timer.now().duration_since(begin) < single_iter_duration {
                        (*guard) += 1;
                        loop_result += 1;
                    }

                    hold_time += timer.now().duration_since(begin);
                });
            }
            println!("Thread {} finished with result {}", id, loop_result);

            return Record {
                id,
                cpu_id: id % num_cpu,
                thread_num: num_thread,
                cpu_num: num_cpu,
                loop_count: loop_result,
                num_acquire,
                hold_time,
                combine_time: lock_type.get_current_thread_combining_time(),
                locktype: lock_type.clone(),
            };
        })
        .unwrap()
}

#[derive(Debug, Parser)]
#[clap(name = "lock counter benchmark", version)]
/// Benchmark Utility
pub struct App {
    #[clap(flatten)]
    global_opts: GlobalOpts,
    #[clap(subcommand)]
    lock_target: LockTarget,
}

#[derive(Debug, Subcommand)]
enum LockTarget {
    /// Benchmark Flat-Combining Skiplist
    FcSL,
    /// Benchmark Flat-Combining Lock
    FcLock,
    /// Benchmark Flat-Combining Fair (Banning) Lock
    FcFairBanLock,
    /// Benchmark Flat-Combining Fair (Banning & Combiner Slice) Lock
    FcFairBanSliceLock,
    /// Benchmark Spinlock
    SpinLock,
    /// Benchmark Mutex
    Mutex,
    /// Benchmark CCSynch
    CCSynch,
    /// Benchmark Remote Core Locking
    RCL,
    /// Benchmark All Lock
    All,
}

#[derive(Debug, Args)]
pub struct GlobalOpts {
    #[clap(long, short, default_values_t = [available_parallelism().unwrap().get()].to_vec())]
    threads: Vec<usize>,
    #[clap(long, short, default_values_t = [available_parallelism().unwrap().get()].to_vec())]
    cpus: Vec<usize>,
    #[clap(long, short, default_value = "../visualization/output")]
    output_path: String,
}

fn main() {
    let app = App::parse();

    assert_eq!(app.global_opts.cpus.len(), app.global_opts.threads.len());

    let output_path = Path::new(app.global_opts.output_path.as_str());

    if output_path.is_dir() {
        // remove the dir
        match remove_dir_all(output_path) {
            Ok(_) => {}
            Err(e) => {
                println!("Error removing output dir: {}", e);
                return;
            }
        }
    }

    match create_dir(output_path) {
        Ok(_) => {}
        Err(e) => {
            println!("Error creating output dir: {}", e);
            return;
        }
    }

    let mut writer = Writer::from_path(output_path.join("output.csv")).unwrap();

    for (ncpu, nthread) in app
        .global_opts
        .threads
        .into_iter()
        .zip(app.global_opts.cpus)
    {
        benchmark(ncpu, nthread, &mut writer, &app.lock_target);
    }
}
