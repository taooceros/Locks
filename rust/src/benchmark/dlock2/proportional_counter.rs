use std::{
    arch::x86_64::__rdtscp,
    fmt::Display,
    hint::{black_box, spin_loop},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread::{self, current, ThreadId},
    time::Duration,
};


use itertools::izip;
use libdlock::dlock2::{DLock2, DLock2Delegate};

use crate::{
    benchmark::{
        bencher::Bencher,
        records::{write_results, Records},
    },
    lock_target::DLock2Target,
};

struct FetchAddDlock2 {
    data: AtomicUsize,
}

unsafe impl DLock2<Data> for FetchAddDlock2 {
    #[inline(always)]
    fn lock(&self, input: Data) -> Data {
        let input = black_box(input);

        if let Data::Input {
            thread_id,
            data: loop_limit,
            stat_response_time,
        } = input
        {
            let timestamp = unsafe {
                if stat_response_time {
                    __rdtscp(&mut 0)
                } else {
                    0
                }
            };

            // it is very important to have black_box here
            let mut loop_limit = loop_limit;

            let mut last_value = 0;

            while loop_limit > 0 {
                last_value = self.data.fetch_add(1, Ordering::AcqRel);
                loop_limit -= 1;
            }

            return Data::Output {
                timestamp,
                is_combiner: current().id() == thread_id,
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
    bencher: &Bencher,
    file_name: &str,
    targets: impl Iterator<Item = &'a DLock2Target>,
    cs_loop: impl Iterator<Item = usize> + Clone,
    non_cs_loop: impl Iterator<Item = usize> + Clone,
    include_lock_free: bool,
) {
    for target in targets {
        let lock = target.to_locktype(0usize, Data::default(), |data: &mut usize, input: Data| {
            let data = black_box(data);
            let input = black_box(input);

            if let Data::Input {
                thread_id,
                data: loop_limit,
                stat_response_time,
            } = input
            {
                let timestamp = unsafe {
                    if stat_response_time {
                        __rdtscp(&mut 0)
                    } else {
                        0
                    }
                };

                // it is very important to have black_box here
                let mut loop_limit = black_box(loop_limit);

                while black_box(loop_limit) > 0 {
                    *data += 1;
                    loop_limit -= 1;
                }

                return Data::Output {
                    timestamp,
                    is_combiner: current().id() == thread_id,
                    data: *data,
                };
            }

            panic!("Invalid input")
        });

        if let Some(lock) = lock {
            let records = start_benchmark(bencher, cs_loop.clone(), non_cs_loop.clone(), lock);
            finish_benchmark(&bencher.output_path, file_name, records);
        }
    }

    if include_lock_free {
        let lock = FetchAddDlock2 {
            data: AtomicUsize::new(0),
        };

        let records = start_benchmark(bencher, cs_loop.clone(), non_cs_loop.clone(), lock);
        finish_benchmark(&bencher.output_path, file_name, records);
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum Data {
    #[default]
    Nothing,
    Input {
        data: usize,
        thread_id: ThreadId,
        stat_response_time: bool,
    },
    Output {
        timestamp: u64,
        is_combiner: bool,
        data: usize,
    },
}

fn start_benchmark(
    bencher: &Bencher,
    cs_loop: impl Iterator<Item = usize> + Clone,
    non_cs_loop: impl Iterator<Item = usize> + Clone,
    lock_target: impl DLock2<Data> + 'static + Display,
) -> Vec<Records> {
    println!("Start benchmark for {}", lock_target);

    let stop_signal = Arc::new(AtomicBool::new(false));
    let lock_ref = Arc::new(lock_target);

    let core_ids = core_affinity::get_core_ids().unwrap();
    let core_ids = core_ids.iter().take(bencher.num_thread);

    // println!("{:?}", bencher);

    thread::scope(move |scope| {
        let handles = izip!(cs_loop.cycle(), non_cs_loop.cycle(), core_ids.cycle(),)
            .take(bencher.num_thread)
            .enumerate()
            .map(|(id, (cs_loop, non_cs_loop, core_id))| {
                let lock_ref = lock_ref.clone();
                let core_id = *core_id;
                let stop_signal = stop_signal.clone();
                let stat_response_time = bencher.stat_response_time;

                scope.spawn(move || {
                    core_affinity::set_for_current(core_id);

                    let stop_signal = stop_signal;
                    let mut combiner_latency = vec![];
                    let mut waiter_latency = vec![];
                    let mut loop_count = 0;
                    let mut num_acquire = 0;
                    let mut aux = 0;

                    let data = Data::Input {
                        data: cs_loop,
                        thread_id: current().id(),
                        stat_response_time,
                    };

                    while !stop_signal.load(Ordering::Acquire) {
                        let begin = if stat_response_time {
                            unsafe { __rdtscp(&mut aux) }
                        } else {
                            0
                        };

                        let output = lock_ref.lock(data);

                        num_acquire += 1;

                        if stat_response_time {
                            if let Data::Output { is_combiner, .. } = output {
                                let end = unsafe { __rdtscp(&mut aux) };
                                if is_combiner {
                                    &mut combiner_latency
                                } else {
                                    &mut waiter_latency
                                }
                                .push(end - begin);
                            } else {
                                panic!("Invalid output");
                            }
                        }

                        loop_count += cs_loop;

                        for i in 0..non_cs_loop {
                            spin_loop();
                            black_box(i);
                        }
                    }

                    Records {
                        id,
                        cpu_id: core_id.id,
                        thread_num: bencher.num_thread,
                        cpu_num: bencher.num_cpu,
                        loop_count: loop_count as u64,
                        num_acquire,
                        cs_length: Duration::from_nanos(cs_loop as u64),
                        non_cs_length: Some(Duration::from_nanos(non_cs_loop as u64)),
                        combiner_latency: combiner_latency,
                        waiter_latency: waiter_latency,
                        hold_time: Default::default(),
                        combine_time: lock_ref.get_combine_time(),
                        locktype: format!("{}", lock_ref),
                        waiter_type: "".to_string(),
                    }
                })
            })
            .collect::<Vec<_>>();

        thread::sleep(Duration::from_secs(bencher.duration));

        stop_signal.store(true, Ordering::Release);

        handles
            .into_iter()
            .map(move |h| h.join().unwrap())
            .collect()
    })
}

fn finish_benchmark<'a>(output_path: &Path, file_name: &str, records: Vec<Records>) {
    write_results(output_path, file_name, &records);

    // for record in records.clone() {
    //     println!("{}", record.loop_count);
    // }

    let total_loop_count: u64 = records.iter().map(|r| r.loop_count).sum();

    println!("Total loop count: {}", total_loop_count);
}
