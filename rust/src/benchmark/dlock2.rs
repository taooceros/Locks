use arrow_ipc::writer::{FileWriter, IpcWriteOptions};
use arrow_ipc::CompressionType;
use core_affinity::CoreId;
use core::num;
use csv::Writer;
use itertools::Itertools;
use libdlock::dlock2::cc::CCSynch;
use libdlock::dlock2::fc::fclock::FCLock;
use libdlock::dlock2::mutex::DLock2Mutex;
use libdlock::dlock2::spinlock::DLock2SpinLock;
use libdlock::dlock2::DLock2;
use std::cell::{OnceCell, RefCell};
use std::fmt::Display;
use std::fs::File;
use std::hint::black_box;
use std::path::Path;
use std::thread::{available_parallelism, current};
use zstd::stream::AutoFinishEncoder;

use std::{
    sync::{atomic::*, Arc},
    thread,
    time::Duration,
};

use crate::benchmark::helper::create_plain_writer;
use crate::benchmark::records::{Records, RecordsBuilder};

use histo::Histogram;
use libdlock::{
    dlock::guard::DLockGuard,
    dlock::{BenchmarkType, DLock},
};
use quanta::Clock;

use super::bencher::LockBenchInfo;

thread_local! {
    static WRITER: OnceCell<
        RefCell<
            Writer<AutoFinishEncoder<'static, File, Box<dyn FnMut(Result<File, std::io::Error>) + Send>>>,
        >,
        > = OnceCell::new();
}

pub fn job(lockdata: &mut u64, data: u64) -> u64 {
    let lockdata = black_box(lockdata);
    let mut data = black_box(data);

    while data > 0 {
        *lockdata += 1;
        data -= 1;
    }
    *lockdata
}

pub fn benchmark_dlock2(info: LockBenchInfo<u64>) {
    let lock1 = Arc::new(FCLock::new(0u64, job));
    let lock2 = Arc::new(CCSynch::new(0u64, job));
    let lock3 = Arc::new(DLock2Mutex::new(0u64, job));
    let lock4 = Arc::new(DLock2SpinLock::new(0u64, job));

    start_bench(lock1, &info);
    start_bench(lock2, &info);
    start_bench(lock3, &info);
    start_bench(lock4, &info);
}

fn start_bench<F, L>(lock: Arc<L>, info: &LockBenchInfo<'_, u64>)
where
    L: DLock2<u64, F> + 'static,
    F: Fn(&mut u64, u64) -> u64 + Send + Sync,
{
    let num_thread = available_parallelism().unwrap().get();

    let mut handles = Vec::new();

    let mut records = Vec::new();
    static STOP: AtomicBool = AtomicBool::new(false);

    STOP.store(false, Ordering::Release);

    for (id, core_id) in core_affinity::get_core_ids()
        .unwrap()
        .iter()
        .cycle()
        .enumerate()
        .take(num_thread)
    {
        let lock = lock.clone();
        let record_response_time = false;
        let cs_duration = Duration::from_nanos(100);
        let non_cs_duration = Duration::from_nanos(0);
        let id = id;
        let core_id = *core_id;
        let handle = thread::spawn(move || {
            thread_job(
                id,
                core_id,
                num_thread,
                num_thread,
                &STOP,
                record_response_time,
                lock,
                cs_duration,
                non_cs_duration,
            )
        });
        handles.push(handle);
    }

    thread::sleep(Duration::from_secs(info.duration));
    STOP.store(true, Ordering::Release);

    for handle in handles {
        records.push(handle.join().unwrap());
    }

    assert!(records.len() == num_thread);

    println!(
        "Lock count {} Loop count {}",
        lock.lock(0),
        records.iter().map(|r| r.loop_count).sum::<u64>()
    );
}

fn thread_job<L, F>(
    id: usize,
    core_id: CoreId,
    num_thread: usize,
    num_cpu: usize,
    stop: &'static AtomicBool,
    record_response_time: bool,
    lock_type: Arc<L>,
    cs_duration: Duration,
    non_cs_duration: Duration,
) -> Records
where
    L: DLock2<u64, F>,
    F: Fn(&mut u64, u64) -> u64 + Send + Sync,
{
    core_affinity::set_for_current(core_id);
    let timer = Clock::new();

    let mut loop_result = 0u64;
    let mut num_acquire = 0u64;
    let mut hold_time = Duration::ZERO;

    let mut respone_time_start = timer.now();

    let mut response_times = Vec::new();
    let mut is_combiners = Vec::new();
    let thread_id = current().id();

    while !stop.load(Ordering::Acquire) {
        // critical section

        const LOOP_LIMIT: u64 = 1000;

        lock_type.lock(LOOP_LIMIT);

        num_acquire += 1;
        loop_result += LOOP_LIMIT;
    }

    println!("{:?} finished: {}", thread_id, loop_result);

    return Records {
        id,
        cpu_id: id % num_cpu,
        thread_num: num_thread,
        cpu_num: num_cpu,
        loop_count: loop_result,
        cs_length: cs_duration,
        non_cs_length: (non_cs_duration > Duration::ZERO).then(|| non_cs_duration),
        num_acquire,
        hold_time,
        is_combiner: if record_response_time {
            Some(is_combiners)
        } else {
            None
        },
        response_times: if record_response_time {
            Some(response_times)
        } else {
            None
        },
        ..Default::default()
    };
}
