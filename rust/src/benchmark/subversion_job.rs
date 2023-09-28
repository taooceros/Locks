use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};

use libdlock::{dlock::{BenchmarkType, DLock}, guard::DLockGuard};
use thread_priority::ThreadPriority;

use super::Record;

pub fn subversion_benchmark(
    lock_type: Arc<BenchmarkType<u64>>,
    id: usize,
    num_thread: usize,
    num_cpu: usize,
    stop: &'static AtomicBool,
) -> Record {
    core_affinity::set_for_current(core_affinity::CoreId { id: id % num_cpu });
    let mut loop_result = 0u64;
    let mut num_acquire = 0u64;
    let mut hold_time = Duration::ZERO;

    if id % 2 == 0 {
        ThreadPriority::Min
    } else {
        ThreadPriority::Max
    }
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