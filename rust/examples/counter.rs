use std::{
    fmt::Debug,
    fs::{create_dir, remove_dir_all, File},
    iter::repeat,
    num::NonZeroI64,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, available_parallelism, JoinHandle},
    time::Duration,
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use csv::Writer;
use itertools::Itertools;
use quanta::Clock;
use strum::{Display, EnumIter, IntoEnumIterator};

use dlock::{
    ccsynch::CCSynch,
    ccsynch_fair_ban::CCBan,
    dlock::{BenchmarkType, DLock, DLockType},
    fc::fclock::FcLock,
    fc_fair_ban::FcFairBanLock,
    fc_fair_ban_slice::FcFairBanSliceLock,
    guard::DLockGuard,
    parker::{block_parker::BlockParker, spin_parker::SpinParker, Parker},
    rcl::{rcllock::RclLock, rclserver::RclServer},
    u_scl::USCL,
};
use dlock::{fc_fair_skiplist::FcSL, spin_lock::SpinLock};

use serde::Serialize;
use serde_with::serde_as;
use serde_with::DurationMilliSeconds;

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

fn benchmark(
    num_cpu: usize,
    num_thread: usize,
    writer: &mut Writer<File>,
    target: Option<LockTarget>,
    waiter: WaiterType,
    duration: u64,
) {
    let targets = extract_targets(waiter, target);

    for target in targets {
        if let Some(lock) = target {
            inner_benchmark(Arc::new(lock), num_cpu, num_thread, writer, duration);
        }
    }

    if matches!(target, Some(LockTarget::DLock(DLockTarget::RCL)) | None) {
        match waiter {
            WaiterType::Spin => bench_rcl::<SpinParker>(num_cpu, num_thread, writer, duration),
            WaiterType::Block => bench_rcl::<BlockParker>(num_cpu, num_thread, writer, duration),
            WaiterType::All => {
                bench_rcl::<SpinParker>(num_cpu, num_thread, writer, duration);
                bench_rcl::<BlockParker>(num_cpu, num_thread, writer, duration)
            }
        }
    }

    println!("Benchmark finished");
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

fn bench_rcl<P>(num_cpu: usize, num_thread: usize, writer: &mut Writer<File>, duration: u64)
where
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
    );
}

fn inner_benchmark(
    lock_type: Arc<BenchmarkType<u64>>,
    num_cpu: usize,
    num_thread: usize,
    writer: &mut Writer<File>,
    duration: u64,
) {
    static STOP: AtomicBool = AtomicBool::new(false);

    STOP.store(false, Ordering::Release);

    let threads = (0..num_thread)
        .map(|id| benchmark_num_threads(lock_type.clone(), id, num_thread, num_cpu, &STOP))
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

fn benchmark_num_threads(
    lock_type: Arc<BenchmarkType<u64>>,
    id: usize,
    num_thread: usize,
    num_cpu: usize,
    stop: &'static AtomicBool,
) -> JoinHandle<Record> {
    let lock_type = lock_type.clone();

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
            let timer = Clock::new();

            let mut loop_result = 0u64;
            let mut num_acquire = 0u64;
            let mut hold_time = Duration::ZERO;

            while !stop.load(Ordering::Acquire) {
                // critical section

                lock_type.lock(|mut guard: DLockGuard<u64>| {
                    num_acquire += 1;
                    let begin = timer.now();

                    while timer.now() - begin < single_iter_duration {
                        (*guard) += 1;
                        loop_result += 1;
                    }
                    hold_time += timer.now().duration_since(begin);
                });
                
                // non-critical section
                thread::sleep(Duration::from_micros(10));
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
                locktype: lock_type.lock_name(),
                waiter_type: lock_type.parker_name().to_string(),
            };
        })
        .unwrap()
}

#[derive(Debug, Parser)]
#[clap(name = "lock counter benchmark", version)]
/// Benchmark Utility
pub struct App {
    #[command(subcommand)]
    lock_target: Option<LockTarget>,
    #[command(flatten)]
    global_opts: GlobalOpts,
}

#[derive(Debug, Clone, Copy, ValueEnum, Display, Serialize)]
enum WaiterType {
    Spin,
    Block,
    All,
}

#[derive(Debug, Subcommand, EnumIter, Clone, Copy)]
enum DLockTarget {
    /// Benchmark Flat-Combining Skiplist
    FcSL,
    /// Benchmark Flat-Combining Lock
    FcLock,
    /// Benchmark Flat-Combining Fair (Banning) Lock
    FcFairBanLock,
    /// Benchmark Flat-Combining Fair (Banning & Combiner Slice) Lock
    FcFairBanSliceLock,

    /// Benchmark CCSynch
    CCSynch,
    /// Benchmark CCSynch (Ban)
    CCBan,
    /// Benchmark Remote Core Locking
    RCL,
}

#[derive(Debug, Subcommand, Clone, Copy)]
enum LockTarget {
    #[command(flatten)]
    DLock(DLockTarget),
    /// Benchmark Mutex
    Mutex,
    /// Benchmark Spinlock
    SpinLock,
    /// Benchmark U-SCL
    USCL,
}

enum LockTargetIterState {
    DLock(DLockTargetIter),
    Mutex,
    SpinLock,
    USCL,
    Stop,
}

struct LockTargetIter {
    state: LockTargetIterState,
}

impl Iterator for LockTargetIter {
    type Item = LockTarget;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.state {
            LockTargetIterState::DLock(iter) => {
                if let Some(dlock) = iter.next() {
                    return Some(LockTarget::DLock(dlock));
                } else {
                    self.state = LockTargetIterState::Mutex;
                    return self.next();
                }
            }
            LockTargetIterState::Mutex => {
                self.state = LockTargetIterState::SpinLock;
                return Some(LockTarget::Mutex);
            }
            LockTargetIterState::SpinLock => {
                self.state = LockTargetIterState::USCL;
                return Some(LockTarget::SpinLock);
            }
            LockTargetIterState::USCL => {
                self.state = LockTargetIterState::Stop;
                return Some(LockTarget::USCL);
            }
            LockTargetIterState::Stop => {
                self.state = LockTargetIterState::DLock(DLockTarget::iter());
                return None;
            }
        }
    }
}

impl IntoEnumIterator for LockTarget {
    type Iterator = LockTargetIter;

    fn iter() -> Self::Iterator {
        LockTargetIter {
            state: LockTargetIterState::DLock(DLockTarget::iter()),
        }
    }
}

impl LockTarget {
    pub fn to_locktype<P>(&self) -> Option<BenchmarkType<u64>>
    where
        P: Parker + 'static,
        BenchmarkType<u64>: From<DLockType<u64, P>>,
    {
        let locktype: DLockType<u64, P> = match self {
            LockTarget::DLock(DLockTarget::FcSL) => FcSL::new(0u64).into(),
            LockTarget::DLock(DLockTarget::FcLock) => FcLock::new(0u64).into(),
            LockTarget::DLock(DLockTarget::FcFairBanLock) => FcFairBanLock::new(0u64).into(),
            LockTarget::DLock(DLockTarget::FcFairBanSliceLock) => {
                FcFairBanSliceLock::new(0u64).into()
            }
            LockTarget::DLock(DLockTarget::CCSynch) => CCSynch::new(0u64).into(),
            LockTarget::DLock(DLockTarget::CCBan) => CCBan::new(0u64).into(),
            // RCL requires) special treatment
            LockTarget::DLock(DLockTarget::RCL) => return None,
            LockTarget::SpinLock => {
                return Some(BenchmarkType::OtherLocks(SpinLock::new(0u64).into()))
            }
            LockTarget::Mutex => return Some(BenchmarkType::OtherLocks(Mutex::new(0u64).into())),
            LockTarget::USCL => return Some(BenchmarkType::OtherLocks(USCL::new(0u64).into())),
        };

        Some(locktype.into())
    }
}

#[derive(Debug, Args)]
pub struct GlobalOpts {
    #[arg(global = true, num_args(0..), value_delimiter = ',', value_terminator("."), long, short, default_values_t = [available_parallelism().unwrap().get()].to_vec())]
    threads: Vec<usize>,
    #[arg(global = true, num_args(0..), value_delimiter = ',', value_terminator("."), long, short, default_values_t = [available_parallelism().unwrap().get()].to_vec())]
    cpus: Vec<usize>,
    #[arg(global = true, long, short, default_value = "../visualization/output")]
    output_path: String,
    #[arg(global = true, long, short, default_value = "all")]
    waiter: WaiterType,
    #[arg(global = true, long, short, default_value = "5")]
    duration: u64,
}

fn main() {
    let mut app = App::parse();

    if app.global_opts.cpus.len() != 1 {
        assert_eq!(app.global_opts.cpus.len(), app.global_opts.threads.len());
    }

    if app.global_opts.cpus.len() == 1 {
        app.global_opts.cpus = repeat(app.global_opts.cpus[0])
            .take(app.global_opts.threads.len())
            .collect();
    }

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
        .cpus
        .into_iter()
        .zip(app.global_opts.threads)
    {
        benchmark(
            ncpu,
            nthread,
            &mut writer,
            app.lock_target,
            app.global_opts.waiter,
            app.global_opts.duration,
        )
    }
}
