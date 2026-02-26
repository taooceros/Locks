use std::{
    arch::x86_64::__rdtscp,
    hint::black_box,
    sync::Arc,
    thread::current,
};

use crate::{
    benchmark::bencher::Bencher,
    lock_target::DLock2Target,
};

use super::counter_common::{finish_benchmark, start_benchmark, Data};

/// Array size for the protected data. 4096 u64s = 32 KiB, fits comfortably
/// in L1 data cache (typically 32-48 KiB). Each CS invocation touches
/// `cs_loop` consecutive elements, so the cache footprint scales with CS.
const ARRAY_SIZE: usize = 4096;

pub fn counter_array<'a>(
    bencher: &Bencher,
    file_name: &str,
    targets: impl Iterator<Item = &'a DLock2Target>,
    cs_loop: impl Iterator<Item = u64> + Clone,
    non_cs_loop: impl Iterator<Item = u64> + Clone,
    _include_lock_free: bool,
    stat_hold_time: bool,
) {
    for target in targets {
        let lock = target.to_locktype(
            [0u64; ARRAY_SIZE],
            Data::default(),
            #[inline(never)]
            move |data: &mut [u64; ARRAY_SIZE], input: Data| {
                let input = input;

                if let Data::Input {
                    thread_id,
                    data: mut loop_limit,
                } = input
                {
                    let timestamp = unsafe {
                        if stat_hold_time {
                            __rdtscp(&mut 0)
                        } else {
                            0
                        }
                    };

                    // Each iteration touches a distinct u64 in the array,
                    // wrapping around. CS=100 means 100 different u64s
                    // (800 bytes, ~13 cache lines) are accessed.
                    let mut idx = 0usize;
                    while loop_limit > 0 {
                        data[idx % ARRAY_SIZE] =
                            black_box(data[idx % ARRAY_SIZE]).wrapping_add(1);
                        idx += 1;
                        loop_limit -= 1;
                    }

                    let hold_time = if stat_hold_time {
                        let end = unsafe { __rdtscp(&mut 0) };
                        end - timestamp
                    } else {
                        0
                    };

                    // Sum first element as representative output value.
                    return Data::Output {
                        hold_time,
                        is_combiner: current().id() == thread_id,
                        data: data[0] as usize,
                    };
                }

                panic!("Invalid input")
            },
        );

        if let Some(lock) = lock {
            let lock = Arc::new(lock);

            let records = start_benchmark(
                bencher,
                stat_hold_time,
                cs_loop.clone(),
                non_cs_loop.clone(),
                lock.clone(),
            );
            finish_benchmark(&bencher.output_path, file_name, &lock.to_string(), records);
        }
    }
}
