use std::{
    arch::x86_64::__rdtscp,
    fmt::Display,
    hint::black_box,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::{self, current, ThreadId},
    time::Duration,
};

use libdlock::dlock2::DLock2;
use rand::Rng;

use crate::{
    benchmark::{
        bencher::Bencher,
        records::{write_results, Records},
    },
    lock_target::DLock2Target,
};

pub struct AtomicF64 {
    storage: AtomicU64,
}
impl AtomicF64 {
    pub fn new(value: f64) -> Self {
        let as_u64 = value.to_bits();
        Self {
            storage: AtomicU64::new(as_u64),
        }
    }
    pub fn store(&self, value: f64, ordering: Ordering) {
        let as_u64 = value.to_bits();
        self.storage.store(as_u64, ordering)
    }
    pub fn load(&self, ordering: Ordering) -> f64 {
        let as_u64 = self.storage.load(ordering);
        f64::from_bits(as_u64)
    }

    pub fn compare_exchange(
        &self,
        current: f64,
        new: f64,
        success: Ordering,
        failure: Ordering,
    ) -> Result<f64, f64> {
        let current_as_u64 = current.to_bits();
        let new_as_u64 = new.to_bits();
        match self
            .storage
            .compare_exchange(current_as_u64, new_as_u64, success, failure)
        {
            Ok(_) => Ok(current),
            Err(actual_as_u64) => {
                let actual = f64::from_bits(actual_as_u64);
                Err(actual)
            }
        }
    }

    pub fn compare_exchange_weak(
        &self,
        current: f64,
        new: f64,
        success: Ordering,
        failure: Ordering,
    ) -> Result<f64, f64> {
        let current_as_u64 = current.to_bits();
        let new_as_u64 = new.to_bits();
        match self
            .storage
            .compare_exchange_weak(current_as_u64, new_as_u64, success, failure)
        {
            Ok(_) => Ok(current),
            Err(actual_as_u64) => {
                let actual = f64::from_bits(actual_as_u64);
                Err(actual)
            }
        }
    }
}

pub struct FetchAndMultiplyDLock2 {
    data: AtomicF64,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum Data {
    #[default]
    Nothing,
    Input {
        data: f64,
        thread_id: ThreadId,
    },
    Output {
        timestamp: u64,
        is_combiner: bool,
        data: f64,
    },
}

impl FetchAndMultiplyDLock2 {
    pub fn new(value: f64) -> Self {
        Self {
            data: AtomicF64::new(value),
        }
    }
}

impl Display for FetchAndMultiplyDLock2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LockFree (Fetch & Multiply)")
    }
}

unsafe impl DLock2<Data> for FetchAndMultiplyDLock2 {
    fn lock(&self, input: Data) -> Data {
        if let Data::Input { thread_id: _, data } = input {
            // compare and exchange loop for fetch and multiply self.data

            let mut current = self.data.load(Ordering::Acquire);

            loop {
                let new = current * data;
                match self.data.compare_exchange_weak(
                    current,
                    new,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        return Data::Output {
                            timestamp: 0,
                            is_combiner: false,
                            data: new,
                        }
                    }
                    Err(actual) => current = actual,
                }
            }
        } else {
            panic!("Invalid input")
        }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        None
    }
}

pub fn fetch_and_multiply<'a>(
    bencher: &Bencher,
    targets: impl Iterator<Item = &'a DLock2Target>,
    include_lock_free: bool,
) {
    for target in targets {
        let stat_response_time = bencher.stat_response_time;

        let lock = target.to_locktype(1.0, Data::default(), move |data: &mut f64, input: Data| {
            let timestamp = unsafe {
                if stat_response_time {
                    __rdtscp(&mut 0)
                } else {
                    0
                }
            };

            if let Data::Input {
                thread_id,
                data: multiplier,
            } = input
            {
                let old_value = *data;
                *data *= multiplier;

                return Data::Output {
                    timestamp,
                    is_combiner: current().id() == thread_id,
                    data: old_value,
                };
            }

            panic!("Invalid input")
        });

        if let Some(lock) = lock {
            let lock = Arc::new(lock);

            let records = start_benchmark(bencher, lock.clone());
            finish_benchmark(&bencher.output_path, "FetchAndMultiply", records, lock);
        }
    }

    if include_lock_free {
        let lock = Arc::new(FetchAndMultiplyDLock2 {
            data: AtomicF64::new(1.0),
        });

        let records = start_benchmark(bencher, lock.clone());
        finish_benchmark(
            &bencher.output_path,
            if bencher.stat_response_time {
                "FetchAndMultiply (latency)"
            } else {
                "FetchAndMultiply"
            },
            records,
            lock,
        );
    }
}

fn start_benchmark<'a>(
    bencher: &Bencher,
    lock_target: Arc<impl DLock2<Data> + 'a + Display>,
) -> Vec<Records> {
    println!("Start benchmark for {}", lock_target);

    let stop_signal = Arc::new(AtomicBool::new(false));
    let lock_ref = lock_target;

    let core_ids = core_affinity::get_core_ids().unwrap();
    let core_ids = core_ids.iter().take(bencher.num_thread);

    // println!("{:?}", bencher);

    thread::scope(move |scope| {
        let handles = core_ids
            .cycle()
            .take(bencher.num_thread)
            .enumerate()
            .map(|(id, core_id)| {
                let lock_ref = lock_ref.clone();
                let core_id = *core_id;
                let stop_signal = stop_signal.clone();
                let stat_response_time = bencher.stat_response_time;

                scope.spawn(move || {
                    core_affinity::set_for_current(core_id);

                    let stop_signal = black_box(stop_signal);
                    let mut waiter_latency = vec![];
                    let mut combiner_latency = vec![];
                    let mut loop_count = 0;
                    let mut num_acquire = 0;
                    let mut aux = 0;

                    let data = Data::Input {
                        data: 1.000001,
                        thread_id: current().id(),
                    };

                    let mut rng = rand::thread_rng();

                    while !stop_signal.load(Ordering::Acquire) {
                        let begin = if stat_response_time {
                            unsafe { __rdtscp(&mut aux) }
                        } else {
                            0
                        };

                        let output = lock_ref.lock(data);

                        num_acquire += 1;

                        if stat_response_time {
                            let end = unsafe { __rdtscp(&mut aux) };
                            if let Data::Output { is_combiner, .. } = output {
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

                        loop_count += 1;

                        // random loop
                        let non_cs_loop = rng.gen_range(1..8);

                        for i in 0..non_cs_loop {
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
                        combiner_latency,
                        waiter_latency,
                        hold_time: Default::default(),
                        combine_time: lock_ref.get_combine_time(),
                        locktype: format!("{}", lock_ref),
                        waiter_type: "".to_string(),
                        ..Default::default()
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
    records: Vec<Records>,
    lock_target: Arc<impl DLock2<Data> + 'static + Display>,
) {
    write_results(output_path, file_name, &records);

    // for record in records.clone() {
    //     println!("{}", record.loop_count);
    // }

    let total_loop_count: u64 = records.iter().map(|r| r.loop_count).sum();

    println!("Total loop count: {}", total_loop_count);

    let data = Data::Input {
        data: 1.0,
        thread_id: current().id(),
    };

    let result = lock_target.lock(data);

    if let Data::Output { data, .. } = result {
        println!("final result: {}", data);
    }
}
