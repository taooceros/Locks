use csv::Writer;
use std::cell::{OnceCell, RefCell};
use std::fs::File;
use zstd::stream::AutoFinishEncoder;
use zstd::Encoder;

use std::{
    sync::{atomic::*, Arc},
    thread,
    time::Duration,
};

use crate::benchmark::{helper::create_zstd_writer, Record};

use histo::Histogram;
use libdlock::{
    dlock::{BenchmarkType, DLock},
    guard::DLockGuard,
};
use quanta::{Clock, Instant};

use super::bencher::LockBenchInfo;
use super::Records;

thread_local! {
    static WRITER: OnceCell<
        RefCell<
            Writer<AutoFinishEncoder<'static, File, Box<dyn FnMut(Result<File, std::io::Error>) + Send>>>,
        >,
        > = OnceCell::new();
}

pub fn counter_proportional(info: LockBenchInfo<u64>) {
    WRITER.with(|cell| {
        println!("Start Proposional Counter for {}", info.lock_type);

        let mut writer = unsafe {
            cell.get_or_init(|| {
                RefCell::new(Writer::from_writer(
                    create_zstd_writer(info.output_path.join("proposion_counter.csv.zst"))
                        .expect("Failed to create writer"),
                ))
            })
            .borrow_mut()
        };

        let (num_thread, num_cpu, lock_type) =
            (info.num_thread, info.num_cpu, info.lock_type.clone());

        static STOP: AtomicBool = AtomicBool::new(false);

        STOP.store(false, Ordering::Release);

        let mut results: Vec<Records> = Vec::new();

        let handles = (0..info.num_thread)
            .map(|id| {
                let lock_type = lock_type.clone();
                thread::Builder::new()
                    .name(format!("Thread {}", id))
                    .spawn(move || {
                        thread_job(
                            id,
                            num_thread,
                            num_cpu,
                            &STOP,
                            info.stat_response_time,
                            lock_type,
                        )
                    })
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

        for result in results.iter().flat_map(|r| r.to_records()) {
            writer.serialize(result).unwrap();
        }

        let total_count: u64 = results.iter().map(|r| r.loop_count).sum();

        if info.verbose {
            let mut histogram = Histogram::with_buckets(5);
            for result in results.iter() {
                histogram.add(result.loop_count);
            }

            println!("{}", histogram);
        } else {
            results.iter().for_each(|r| println!("{}", r.loop_count));
        }

        lock_type.lock(|guard: DLockGuard<u64>| {
            assert_eq!(
                *guard, total_count,
                "Total counter is not matched with lock value {}, but thread local loop sum {}",
                *guard, total_count
            );
        });

        println!(
            "Finish OneThreeCounter for {}: Total Counter {}",
            lock_type, total_count
        );
    })
}

fn thread_job(
    id: usize,
    num_thread: usize,
    num_cpu: usize,
    stop: &'static AtomicBool,
    record_response_time: bool,
    lock_type: Arc<BenchmarkType<u64>>,
) -> Records {
    core_affinity::set_for_current(core_affinity::CoreId { id: id % num_cpu });
    let single_iter_duration: Duration = Duration::from_micros((10 * (id + 1)).try_into().unwrap());
    let timer = Clock::new();

    let mut loop_result = 0u64;
    let mut num_acquire = 0u64;
    let mut hold_time = Duration::ZERO;

    let mut respone_time_start = timer.now();

    let mut response_times = Vec::new();

    while !stop.load(Ordering::Acquire) {
        // critical section

        if record_response_time {
            respone_time_start = timer.now();
        }

        lock_type.lock(|mut guard: DLockGuard<u64>| {
            if record_response_time {
                let respone_time = timer.now().duration_since(respone_time_start);
                response_times.push(Some(respone_time));
            }

            num_acquire += 1;
            let begin = timer.now();

            while timer.now() - begin < single_iter_duration {
                (*guard) += 1;
                loop_result += 1;
            }
            hold_time += timer.now().duration_since(begin);
        });
    }

    return Records {
        id,
        cpu_id: id % num_cpu,
        thread_num: num_thread,
        cpu_num: num_cpu,
        loop_count: loop_result,
        job_length: single_iter_duration,
        num_acquire,
        hold_time,
        response_times: if record_response_time {
            Some(response_times)
        } else {
            None
        },
        #[cfg(feature = "combiner_stat")]
        combine_time: lock_type.get_current_thread_combining_time(),
        locktype: lock_type.lock_name(),
        waiter_type: lock_type.parker_name().to_string(),
        ..Default::default()
    };
}
