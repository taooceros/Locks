use std::{
    arch::x86_64::__rdtscp,
    fmt::Display,
    hint::black_box,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread::current,
};

use libdlock::dlock2::DLock2;

use crate::lock_target::DLock2Target;

use super::counter_common::{finish_benchmark, start_benchmark, Data};

struct FetchAddDlock2 {
    data: AtomicUsize,
}

unsafe impl DLock2<Data> for FetchAddDlock2 {
    #[inline(always)]
    fn lock(&self, input: Data) -> Data {
        let input = black_box(input);

        if let Data::Input {
            thread_id: _,
            data: loop_limit,
        } = input
        {
            // it is very important to have black_box here
            let mut loop_limit = loop_limit;

            let mut last_value = 0;

            while loop_limit > 0 {
                last_value = self.data.fetch_add(1, Ordering::AcqRel);
                loop_limit -= 1;
            }

            return Data::Output {
                hold_time: 0,
                is_combiner: true,
                data: last_value + 1,
            };
        }

        panic!("Invalid input")
    }

    fn get_combine_time(&self) -> std::option::Option<u64> {
        None
    }
}

impl Display for FetchAddDlock2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LockFree (Fetch&Add)")
    }
}

pub fn proportional_counter<'a>(
    bencher: &crate::benchmark::bencher::Bencher,
    file_name: &str,
    targets: impl Iterator<Item = &'a DLock2Target>,
    cs_loop: impl Iterator<Item = u64> + Clone,
    non_cs_loop: impl Iterator<Item = u64> + Clone,
    include_lock_free: bool,
    stat_hold_time: bool,
) {
    for target in targets {
        let lock = target.to_locktype(
            0usize,
            Data::default(),
            #[inline(never)]
            move |data: &mut usize, input: Data| {
                let data = data;
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

                    while loop_limit > 0 {
                        *black_box(&mut *data) += 1;
                        loop_limit -= 1;
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
                        data: *data,
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

    if include_lock_free {
        let lock = FetchAddDlock2 {
            data: AtomicUsize::new(0),
        };

        let records = start_benchmark(
            bencher,
            stat_hold_time,
            cs_loop.clone(),
            non_cs_loop.clone(),
            Arc::new(lock),
        );
        finish_benchmark(&bencher.output_path, file_name, "Fetch&Add", records);
    }
}
