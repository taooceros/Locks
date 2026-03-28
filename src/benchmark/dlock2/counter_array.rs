use std::{arch::x86_64::__rdtscp, hint::black_box, sync::Arc, thread::current};

use crate::{benchmark::bencher::Bencher, lock_target::DLock2Target};

use super::counter_common::{finish_benchmark, start_benchmark, Data};

pub fn counter_array<'a>(
    bencher: &Bencher,
    file_name: &str,
    targets: impl Iterator<Item = &'a DLock2Target>,
    cs_loop: impl Iterator<Item = u64> + Clone,
    non_cs_loop: impl Iterator<Item = u64> + Clone,
    _include_lock_free: bool,
    stat_hold_time: bool,
    array_size: usize,
    random_access: bool,
) {
    for target in targets {
        let lock = target.to_locktype(
            vec![0u64; array_size],
            Data::default(),
            #[inline(never)]
            move |data: &mut Vec<u64>, input: Data| {
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

                    let len = data.len();

                    if random_access {
                        // Xorshift32 PRNG for random index generation.
                        // Seeded from loop_limit to vary across CS sizes.
                        let mut rng = (loop_limit as u32) | 1;
                        while loop_limit > 0 {
                            rng ^= rng << 13;
                            rng ^= rng >> 17;
                            rng ^= rng << 5;
                            let idx = (rng as usize) % len;
                            data[idx] = black_box(data[idx]).wrapping_add(1);
                            loop_limit -= 1;
                        }
                    } else {
                        // Sequential access (original behavior).
                        let mut idx = 0usize;
                        while loop_limit > 0 {
                            data[idx % len] = black_box(data[idx % len]).wrapping_add(1);
                            idx += 1;
                            loop_limit -= 1;
                        }
                    }

                    let hold_time = if stat_hold_time {
                        let end = unsafe { __rdtscp(&mut 0) };
                        end - timestamp
                    } else {
                        0
                    };

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
