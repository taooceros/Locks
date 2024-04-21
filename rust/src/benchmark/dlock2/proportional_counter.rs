use std::{
    arch::x86_64::__rdtscp,
    fmt::Display,
    hint::black_box,
    iter::zip,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread::{self, current, ThreadId},
    time::Duration,
};

use bitvec::prelude::*;
use itertools::izip;
use libdlock::{
    dlock2::{CombinerStatistics, DLock2},
};

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

    #[cfg(feature = "combiner_stat")]
    fn get_combine_stat(&self) -> std::option::Option<&CombinerStatistics> {
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
    cs_loop: impl Iterator<Item = u64> + Clone,
    non_cs_loop: impl Iterator<Item = u64> + Clone,
    include_lock_free: bool,
    stat_hold_time: bool,
) {
    for target in targets {
        let lock = target.to_locktype(
            0usize,
            Data::default(),
            move |data: &mut usize, input: Data| {
                let data = black_box(data);
                let input = black_box(input);

                if let Data::Input {
                    thread_id,
                    data: loop_limit,
                } = input
                {
                    let timestamp = unsafe {
                        if stat_hold_time {
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

#[derive(Debug, Clone, Copy, Default)]
pub enum Data {
    #[default]
    Nothing,
    Input {
        data: u64,
        thread_id: ThreadId,
    },
    Output {
        hold_time: u64,
        is_combiner: bool,
        data: usize,
    },
}

fn start_benchmark<L>(
    bencher: &Bencher,
    stat_hold_time: bool,
    cs_loop: impl Iterator<Item = u64> + Clone,
    non_cs_loop: impl Iterator<Item = u64> + Clone,
    lock: Arc<L>,
) -> Vec<Records>
where
    L: DLock2<Data> + 'static + Display,
{
    println!("Start benchmark for {}", lock);

    let stop_signal = Arc::new(AtomicBool::new(false));

    let core_ids = core_affinity::get_core_ids().unwrap();
    let core_ids = core_ids.iter().take(bencher.num_thread);

    // println!("{:?}", bencher);

    thread::scope(move |scope| {
        let handles = izip!(cs_loop.cycle(), non_cs_loop.cycle(), core_ids.cycle(),)
            .take(bencher.num_thread)
            .enumerate()
            .map(|(id, (cs_loop, non_cs_loop, core_id))| {
                let lock_ref = lock.clone();
                let core_id = *core_id;
                let stop_signal = stop_signal.clone();
                let stat_response_time = bencher.stat_response_time;

                scope.spawn(move || {
                    core_affinity::set_for_current(core_id);

                    let stop_signal = stop_signal;
                    let mut latencies = vec![];
                    let mut is_combiners: BitVec<usize, Lsb0> = BitVec::new();
                    let mut loop_count = 0;
                    let mut num_acquire = 0;
                    let mut aux = 0;

                    let data = Data::Input {
                        data: cs_loop,
                        thread_id: current().id(),
                    };

                    let mut hold_time = 0;

                    while !stop_signal.load(Ordering::Acquire) {
                        let begin = if stat_response_time {
                            unsafe { __rdtscp(&mut aux) }
                        } else {
                            0
                        };

                        let output = lock_ref.lock(data);

                        num_acquire += 1;

                        if let Data::Output {
                            is_combiner,
                            hold_time: current_hold_time,
                            ..
                        } = output
                        {
                            if stat_response_time {
                                let end = unsafe { __rdtscp(&mut aux) };
                                latencies.push(end - begin);
                                is_combiners.push(is_combiner);
                            }

                            if stat_hold_time {
                                hold_time += current_hold_time;
                            }
                        } else {
                            unreachable!();
                        }

                        loop_count += cs_loop;

                        for i in 0..non_cs_loop {
                            black_box(i);
                        }
                    }

                    // make the branch prediction fail at the end

                    let combiner_count = is_combiners.count_ones();

                    let mut combiner_latency = Vec::with_capacity(combiner_count);
                    let mut waiter_latency =
                        Vec::with_capacity(is_combiners.len() - combiner_count);

                    for (latency, is_combiner) in zip(latencies.iter(), is_combiners.iter()) {
                        if *is_combiner {
                            combiner_latency.push(*latency);
                        } else {
                            waiter_latency.push(*latency);
                        }
                    }

                    let combiner_stat = lock_ref.get_combine_stat();

                    Records {
                        id,
                        cpu_id: core_id.id,
                        loop_count,
                        num_acquire,
                        cs_length: cs_loop,
                        non_cs_length: Some(non_cs_loop),
                        combiner_latency,
                        waiter_latency,
                        hold_time,
                        combine_time: combiner_stat.map(|s| s.combine_time.iter().sum()),
                        locktype: format!("{}", lock_ref),
                        waiter_type: "".to_string(),
                        ..Records::from_bencher(bencher)
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

fn finish_benchmark<'a>(
    output_path: &Path,
    file_name: &str,
    lock_name: &str,
    records: impl AsRef<Vec<Records>>,
) {
    let folder = output_path.join(lock_name);

    if !folder.exists() {
        std::fs::create_dir_all(&folder).unwrap();
    }

    let records = records.as_ref();

    write_results(&folder, file_name, records);

    for record in records.iter() {
        println!("{}", record.loop_count);
    }

    let total_loop_count: u64 = records.iter().map(|r| r.loop_count).sum();

    println!("Total loop count: {}", total_loop_count);
}
