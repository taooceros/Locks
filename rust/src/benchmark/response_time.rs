use crate::benchmark::bencher::LockBenchInfo;
use crate::benchmark::helper::create_writer;
use csv::Writer;
use histo::Histogram;
use libdlock::dlock::{BenchmarkType, DLock};
use libdlock::guard::DLockGuard;
use quanta::Clock;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DurationNanoSeconds;
use std::cell::{OnceCell, RefCell};
use std::fs::File;
use std::iter::Once;
use std::num::{NonZeroI64, NonZeroU64};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

static mut WRITER: OnceCell<RefCell<Writer<File>>> = OnceCell::new();

pub fn benchmark_response_time(info: LockBenchInfo<u64>) {
    println!(
        "Start Respone Time Measure for {}",
        info.lock_type.lock_name()
    );

    let (num_thread, num_cpu, lock_type) = (info.num_thread, info.num_cpu, info.lock_type.clone());

    static STOP: AtomicBool = AtomicBool::new(false);

    STOP.store(false, Ordering::Release);

    let mut results: Vec<Record> = Vec::new();

    let handles = (0..info.num_thread)
        .map(|id| {
            let lock_type = lock_type.clone();
            thread::Builder::new()
                .name(format!("Thread {}", id))
                .spawn(move || thread_job(id, num_thread, num_cpu, &STOP, lock_type))
                .expect("Failed to spawn thread")
        })
        .collect::<Vec<_>>();

    thread::sleep(Duration::from_secs(info.duration));

    STOP.store(true, Ordering::Release);

    let mut i = 0;

    for job in handles {
        let l = job.join();
        match l {
            Ok(l) => {
                results.push(l);
                // println!("{}", l);
            }
            Err(_e) => eprintln!("Error joining thread: {}", i),
        }
        i += 1;
    }

    let file = create_writer(&info.output_path.join("response_times").join(format!(
        "{}-{}-{}-{}.json",
        lock_type.lock_name(),
        lock_type.parker_name(),
        num_cpu,
        num_thread
    )))
    .expect("Failed to create writer");

    serde_json::to_writer(file, &results).unwrap();

    if info.verbose {
        let mut histogram = Histogram::with_buckets(5);
        for result in results.into_iter().flat_map(|r| r.response_times) {
            histogram.add(result.as_micros() as u64);
        }

        println!("{}", histogram);
    }

    println!("Finish Response Time benchmark for {}", lock_type);
}

fn thread_job(
    id: usize,
    num_thread: usize,
    num_cpu: usize,
    stop: &'static AtomicBool,
    lock_type: Arc<BenchmarkType<u64>>,
) -> Record {
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

    let mut response_times = Vec::new();

    while !stop.load(Ordering::Acquire) {
        // critical section

        let begin = timer.now();

        lock_type.lock(|mut guard: DLockGuard<u64>| {
            num_acquire += 1;
            let begin = timer.now();

            while timer.now() - begin < single_iter_duration {
                (*guard) += 1;
                loop_result += 1;
            }
            hold_time += timer.now().duration_since(begin);
        });

        response_times.push(timer.now().duration_since(begin));
    }

    return Record {
        id,
        cpu_id: id % num_cpu,
        thread_num: num_thread,
        cpu_num: num_cpu,
        hold_time,
        response_times,
        #[cfg(feature = "combiner_stat")]
        combine_time: lock_type.get_current_thread_combining_time(),
        locktype: lock_type.lock_name(),
        waiter_type: lock_type.parker_name().to_string(),
    };
}

#[serde_as]
#[derive(Serialize, Deserialize)]
pub struct Record {
    pub id: usize,
    pub cpu_id: usize,
    pub thread_num: usize,
    pub cpu_num: usize,
    #[serde_as(as = "Vec<DurationNanoSeconds<u64>>")]
    pub response_times: Vec<Duration>,
    pub hold_time: Duration,
    #[cfg(feature = "combiner_stat")]
    pub combine_time: Option<NonZeroI64>,
    pub locktype: String,
    pub waiter_type: String,
}
