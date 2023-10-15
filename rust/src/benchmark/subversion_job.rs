use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use libdlock::{
    dlock::{BenchmarkType, DLock},
    guard::DLockGuard,
};
use thread_priority::{ThreadPriority, ThreadPriorityValue};

use crate::benchmark::helper::create_writer;

use super::{bencher::LockBenchInfo, Record};

pub fn subversion_benchmark(info: LockBenchInfo<u64>) {
    println!("Start Subversion for {}", info.lock_type.lock_name());

    let mut writer = create_writer(&info.output_path.join("subversion_counter.csv"))
        .expect("Failed to create writer");

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
        "Finish Subversion for {}: Total Counter {}",
        lock_type, total_count
    );
}

fn thread_job(
    id: usize,
    num_thread: usize,
    num_cpu: usize,
    stop: &'static AtomicBool,
    lock_type: Arc<BenchmarkType<u64>>,
) -> Record {
    core_affinity::set_for_current(core_affinity::CoreId { id: id % num_cpu });
    let mut loop_result = 0u64;
    let mut num_acquire = 0u64;
    let hold_time = Duration::ZERO;

    let priority = if id % 2 == 0 { 0u8 } else { 50u8 };

    // println!("Thread {} started with priority {:?}", id, priority);

    ThreadPriority::Crossplatform(ThreadPriorityValue::try_from(priority).unwrap())
        .set_for_current()
        .unwrap();

    while !stop.load(Ordering::Acquire) {
        // critical section

        lock_type.lock(|mut guard: DLockGuard<u64>| {
            num_acquire += 1;

            (*guard) += 1;
            loop_result += 1;
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
        #[cfg(feature = "combiner_stat")]
        combine_time: lock_type.get_current_thread_combining_time(),
        locktype: lock_type.lock_name(),
        waiter_type: lock_type.parker_name().to_string(),
    };
}
