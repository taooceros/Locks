use std::{
    arch::x86_64::__rdtscp,
    cell::{OnceCell, RefCell},
    fmt::Display,
    hint::{black_box, spin_loop},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::{self, current, ThreadId},
    time::Duration,
};

use arrow_ipc::writer::{FileWriter, IpcWriteOptions};
use itertools::izip;
use libdlock::dlock2::{DLock2, DLock2Delegate};
use rand::Rng;

use crate::{
    benchmark::{
        bencher::Bencher,
        helper::create_plain_writer,
        records::{Records, RecordsBuilder},
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

impl DLock2<f64, Data, fn(&mut f64, Data) -> Data> for FetchAndMultiplyDLock2 {
    fn lock(&self, input: Data) -> Data {
        let mut data = self.data.load(Ordering::Acquire);
        if let Data::Input { thread_id, data } = input {
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
    file_name: &str,
    targets: impl Iterator<Item = &'a DLock2Target>,
    include_lock_free: bool,
) {
    for target in targets {
        let stat_response_time = bencher.stat_response_time;

        let lock = target.to_locktype(0.0, Data::default(), move |data: &mut f64, input: Data| {
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
            let records = start_benchmark(bencher, lock);
            finish_benchmark(&bencher.output_path, file_name, records.iter());
        }
    }

    if include_lock_free {
        let lock = FetchAndMultiplyDLock2 {
            data: AtomicF64::new(0.0),
        };

        let records = start_benchmark::<fn(&mut f64, Data) -> Data>(bencher, lock);
        finish_benchmark(&bencher.output_path, file_name, records.iter());
    }
}

fn start_benchmark<F>(
    bencher: &Bencher,
    lock_target: impl DLock2<f64, Data, F> + 'static + Display,
) -> Vec<Records>
where
    F: DLock2Delegate<f64, Data>,
{
    println!("Start benchmark for {}", lock_target);

    let stop_signal = Arc::new(AtomicBool::new(false));
    let lock_ref = Arc::new(lock_target);

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
                    let mut response_times = vec![];
                    let mut is_combiners = vec![];
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
                            if let Data::Output { is_combiner, .. } = output {
                                is_combiners.push(Some(is_combiner));
                            } else {
                                panic!("Invalid output");
                            }

                            let end = unsafe { __rdtscp(&mut aux) };
                            response_times.push(Some(Duration::from_nanos(end - begin)));
                        }

                        loop_count += 1;

                        // random loop
                        let non_cs_loop = rng.gen_range(1..32);

                        for _ in 0..non_cs_loop {
                            spin_loop()
                        }
                    }

                    Records {
                        id,
                        cpu_id: core_id.id,
                        thread_num: bencher.num_thread,
                        cpu_num: bencher.num_cpu,
                        loop_count: loop_count as u64,
                        num_acquire,
                        cs_length: Duration::from_nanos(0),
                        non_cs_length: Some(Duration::from_nanos(0)),
                        is_combiner: Some(is_combiners),
                        response_times: Some(response_times),
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

fn finish_benchmark<'a>(
    output_path: &Path,
    file_name: &str,
    records: impl Iterator<Item = &'a Records> + Clone,
) {
    write_results(output_path, file_name, records.clone());

    // for record in records.clone() {
    //     println!("{}", record.loop_count);
    // }

    let total_loop_count: u64 = records.clone().map(|r| r.loop_count).sum();

    println!("Total loop count: {}", total_loop_count);
}

fn write_results<'a>(
    output_path: &Path,
    file_name: &str,
    results: impl Iterator<Item = &'a Records>,
) {
    thread_local! {
        static WRITER: OnceCell<RefCell<FileWriter<std::fs::File>>> = OnceCell::new();
    }

    WRITER.with(|cell| {
        let mut writer = cell
            .get_or_init(|| {
                let option =
                    IpcWriteOptions::try_new(8, false, arrow::ipc::MetadataVersion::V5).unwrap();
                // .try_with_compression(Some(CompressionType::ZSTD))
                // .expect("Failed to create compression option");

                RefCell::new(
                    FileWriter::try_new_with_options(
                        create_plain_writer(output_path.join(format!("{file_name}.arrow")))
                            .expect("Failed to create writer"),
                        RecordsBuilder::get_schema(),
                        option,
                    )
                    .expect("Failed to create file writer"),
                )
            })
            .borrow_mut();

        let mut record_builder = RecordsBuilder::default();

        record_builder.extend(results);

        writer
            .write(&record_builder.finish().into())
            .expect("Failed to write");
    });
}
