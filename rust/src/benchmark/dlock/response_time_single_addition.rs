use crate::benchmark::bencher::LockBenchInfo;
use crate::benchmark::helper::create_plain_writer;
use crate::benchmark::old_records::{Records, RecordsBuilder};

use arrow::ipc::writer::{FileWriter, IpcWriteOptions};
use arrow::ipc::CompressionType;

use histo::Histogram;
use libdlock::dlock::{guard::DLockGuard, BenchmarkType, DLock};
use quanta::Clock;

use std::cell::{OnceCell, RefCell};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, current};
use std::time::Duration;

pub fn benchmark_response_time_single_addition(info: LockBenchInfo<u64>) {
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
                .spawn(move || {
                    thread_job(
                        id,
                        num_thread,
                        num_cpu,
                        &STOP,
                        lock_type,
                        info.stat_response_time,
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

    thread_local! {
        static WRITER: OnceCell<RefCell<FileWriter<std::fs::File>>> = OnceCell::new();
    }

    WRITER.with(|cell| {
        let mut writer = cell
            .get_or_init(|| {
                let option = IpcWriteOptions::try_new(8, false, arrow::ipc::MetadataVersion::V5)
                    .unwrap()
                    .try_with_compression(Some(CompressionType::ZSTD))
                    .expect("Failed to create compression option");

                RefCell::new(
                    FileWriter::try_new_with_options(
                        create_plain_writer(
                            info.output_path.join("response_time_single_addition.arrow"),
                        )
                        .expect("Failed to create writer"),
                        RecordsBuilder::get_schema(),
                        option,
                    )
                    .unwrap(),
                )
            })
            .borrow_mut();

        let mut record_builder = RecordsBuilder::default();

        record_builder.extend(results.iter());

        writer
            .write(&record_builder.finish().into())
            .expect("Failed to write");
    });

    if info.verbose {
        let mut histogram = Histogram::with_buckets(5);
        for result in results
            .iter()
            .flat_map(|r| r.response_times.as_ref().unwrap())
        {
            histogram.add(result.as_ref().unwrap().as_nanos().try_into().unwrap());
        }

        println!("{}", histogram);
    }

    println!(
        "Finish Response Time benchmark for {} loop {}",
        lock_type,
        results.iter().map(|r| r.loop_count).sum::<u64>()
    );
}

fn thread_job(
    id: usize,
    num_thread: usize,
    num_cpu: usize,
    stop: &'static AtomicBool,
    lock_type: Arc<BenchmarkType<u64>>,
    stat_response_time: bool,
) -> Records {
    core_affinity::set_for_current(core_affinity::CoreId { id: id % num_cpu });
    let timer = Clock::new();

    let mut num_acquire = 0u64;
    let hold_time = Duration::ZERO;

    let mut response_times = vec![];
    let mut is_combiners = vec![];

    let thread_id = current().id();

    while !stop.load(Ordering::Acquire) {
        // critical section

        let begin = timer.now();
        let mut diff = None;

        let mut is_combiner = false;

        lock_type.lock(|mut guard: DLockGuard<u64>| {
            if stat_response_time {
                let now = timer.now();
                diff = Some(now - begin);
            }
            *guard += 1;

            is_combiner = current().id() == thread_id;
        });
        num_acquire += 1;

        response_times.push(diff);
        is_combiners.push(Some(is_combiner));
    }

    return Records {
        id,
        cpu_id: id % num_cpu,
        thread_num: num_thread,
        cpu_num: num_cpu,
        num_acquire,
        loop_count: num_acquire,
        hold_time,
        is_combiner: Some(is_combiners),
        response_times: Some(response_times),
        #[cfg(feature = "combiner_stat")]
        combine_time: lock_type.get_current_thread_combining_time(),
        locktype: lock_type.lock_name(),
        waiter_type: lock_type.parker_name().to_string(),
        ..Default::default()
    };
}
