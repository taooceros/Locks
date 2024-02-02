use crate::benchmark::bencher::LockBenchInfo;
use crate::benchmark::helper::create_zstd_writer;
use csv::Writer;
use histo::Histogram;
use libdlock::dlock::guard::DLockGuard;
use libdlock::dlock::{BenchmarkType, DLock};
use quanta::Clock;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DurationNanoSeconds;
use std::cell::{OnceCell, RefCell};
use std::fs::File;
use std::num::NonZeroI64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, current};
use std::time::Duration;
use zstd::stream::AutoFinishEncoder;

static mut WRITER: OnceCell<
    RefCell<
        Writer<AutoFinishEncoder<'_, File, Box<dyn FnMut(Result<File, std::io::Error>) + Send>>>,
    >,
> = OnceCell::new();

pub fn benchmark_response_time_one_three_ratio(info: LockBenchInfo<u64>) {
    println!(
        "Start Respone Time Measure for {}",
        info.lock_type.lock_name()
    );

    let (num_thread, num_cpu, lock_type) = (info.num_thread, info.num_cpu, info.lock_type.clone());

    static STOP: AtomicBool = AtomicBool::new(false);

    STOP.store(false, Ordering::Release);

    let mut results: Vec<Records> = Vec::new();

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

    static mut WRITER: OnceCell<
        RefCell<
            Writer<
                AutoFinishEncoder<'_, File, Box<dyn FnMut(Result<File, std::io::Error>) + Send>>,
            >,
        >,
    > = OnceCell::new();

    let mut writer = unsafe {
        WRITER
            .get_or_init(|| {
                RefCell::new(Writer::from_writer(
                    create_zstd_writer(info.output_path.join("response_time_one_three_ratio.csv"))
                        .expect("Failed to create writer"),
                ))
            })
            .borrow_mut()
    };
    for result in results.iter().flat_map(|r| r.to_records()) {
        writer.serialize(result).unwrap();
    }

    if info.verbose {
        let mut histogram = Histogram::with_buckets(5);
        for result in results.into_iter().flat_map(|r| r.response_times) {
            histogram.add(result.as_nanos().try_into().unwrap());
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
) -> Records {
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

    let mut response_times = vec![];
    let mut is_combiners = vec![];

    let thread_id = current().id();

    while !stop.load(Ordering::Acquire) {
        // critical section

        let wait_begin = timer.now();

        let mut is_combiner = false;

        lock_type.lock(|mut guard: DLockGuard<u64>| {
            response_times.push(timer.now().duration_since(wait_begin));

            num_acquire += 1;
            let begin = timer.now();

            while timer.now() - begin < single_iter_duration {
                (*guard) += 1;
                loop_result += 1;
            }

            is_combiner = current().id() == thread_id;

            hold_time += timer.now().duration_since(begin);
        });

        is_combiners.push(is_combiner);
    }

    return Records {
        id,
        cpu_id: id % num_cpu,
        thread_num: num_thread,
        cpu_num: num_cpu,
        hold_time,
        job_length: single_iter_duration,
        is_combiner: is_combiners,
        response_times,
        #[cfg(feature = "combiner_stat")]
        combine_time: lock_type.get_current_thread_combining_time(),
        locktype: lock_type.lock_name(),
        waiter_type: lock_type.parker_name().to_string(),
    };
}

pub struct Records {
    pub id: usize,
    pub cpu_id: usize,
    pub thread_num: usize,
    pub cpu_num: usize,
    pub job_length: Duration,
    pub is_combiner: Vec<bool>,
    pub response_times: Vec<Duration>,
    pub hold_time: Duration,
    pub combine_time: Option<NonZeroI64>,
    pub locktype: String,
    pub waiter_type: String,
}

impl Records {
    pub fn to_records(&self) -> impl Iterator<Item = Record> + '_ {
        self.is_combiner.iter().zip(self.response_times.iter()).map(
            |(is_combiner, response_time)| Record {
                id: self.id,
                cpu_id: self.cpu_id,
                thread_num: self.thread_num,
                cpu_num: self.cpu_num,
                job_length: self.job_length,
                is_combiner: *is_combiner,
                response_times: *response_time,
                hold_time: self.hold_time,
                #[cfg(feature = "combiner_stat")]
                combine_time: self.combine_time,
                locktype: self.locktype.clone(),
                waiter_type: self.waiter_type.clone(),
            },
        )
    }
}

#[serde_as]
#[derive(Serialize, Deserialize)]
pub struct Record {
    pub id: usize,
    pub cpu_id: usize,
    pub thread_num: usize,
    pub cpu_num: usize,
    #[serde_as(as = "DurationNanoSeconds")]
    pub job_length: Duration,
    pub is_combiner: bool,
    #[serde_as(as = "DurationNanoSeconds")]
    pub response_times: Duration,
    #[serde_as(as = "DurationNanoSeconds")]
    pub hold_time: Duration,
    #[cfg(feature = "combiner_stat")]
    pub combine_time: Option<NonZeroI64>,
    pub locktype: String,
    pub waiter_type: String,
}
