use std::{
    sync::{atomic::*, Arc},
    time::Duration,
};

use crate::benchmark::Record;

use libdlock::{
    dlock::{BenchmarkType, DLock},
    guard::DLockGuard,
};
use quanta::Clock;

pub fn one_three_benchmark(
    lock_type: Arc<BenchmarkType<u64>>,
    id: usize,
    num_thread: usize,
    num_cpu: usize,
    stop: &'static AtomicBool,
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
