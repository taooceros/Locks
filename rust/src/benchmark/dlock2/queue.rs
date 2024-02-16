use std::{
    arch::x86_64::__rdtscp,
    cell::{OnceCell, RefCell},
    collections::{LinkedList, VecDeque},
    hint::{black_box, spin_loop},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self},
    time::Duration,
};

use arrow_ipc::writer::{FileWriter, IpcWriteOptions};

use clap::ValueEnum;
use libdlock::dlock2::{DLock2, DLock2Delegate};
use rand::Rng;
use strum::{Display, EnumIter};

use crate::{
    benchmark::{
        bencher::Bencher,
        helper::create_plain_writer,
        records::{Records, RecordsBuilder},
    },
    lock_target::DLock2Target,
};

#[derive(Debug, Clone, ValueEnum, EnumIter, Display)]
pub enum LockFreeQueue {
    MCSQueue,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum QueueData<T: Send> {
    #[default]
    Nothing,
    Push {
        data: T,
    },
    Pop,
    OutputT {
        data: T,
    },
    OutputEmpty,
}

pub unsafe trait ConcurrentQueue<T>: Send + Sync
where
    T: Send,
{
    fn push(&self, value: T);
    fn pop(&self) -> Option<T>;
}

pub trait SequentialQueue<T> {
    fn push(&mut self, value: T);
    fn pop(&mut self) -> Option<T>;
}

unsafe impl<T, L> ConcurrentQueue<T> for L
where
    T: Send,
    L: DLock2<QueueData<T>>,
{
    fn push(&self, value: T) {
        self.lock(QueueData::Push { data: value });
    }

    fn pop(&self) -> Option<T> {
        match self.lock(QueueData::Pop) {
            QueueData::OutputT { data } => Some(data),
            QueueData::OutputEmpty => None,
            _ => panic!("Invalid output"),
        }
    }
}

impl<T: Send> SequentialQueue<T> for VecDeque<T> {
    fn push(&mut self, value: T) {
        self.push_back(value);
    }

    fn pop(&mut self) -> Option<T> {
        self.pop_front()
    }
}

impl<T: Send> SequentialQueue<T> for LinkedList<T> {
    fn push(&mut self, value: T) {
        self.push_back(value);
    }

    fn pop(&mut self) -> Option<T> {
        self.pop_front()
    }
}

pub fn benchmark_queue<'a>(
    bencher: &Bencher,
    targets: impl Iterator<Item = &'a DLock2Target>,
    lock_free_queues: &Vec<LockFreeQueue>,
) {
    for target in targets {
        let lock = target.to_locktype(
            LinkedList::new(),
            QueueData::default(),
            move |queue: &mut LinkedList<u64>, input: QueueData<u64>| match input {
                QueueData::Push { data } => {
                    queue.push(data);
                    QueueData::Nothing
                }
                QueueData::Pop => {
                    let output = queue.pop();
                    match output {
                        Some(data) => QueueData::OutputT { data },
                        None => QueueData::OutputEmpty,
                    }
                }
                _ => panic!("Invalid input"),
            },
        );

        if let Some(lock) = lock {
            let lockname = format!("{}-queue", lock);
            let records = start_benchmark(bencher, lock, &lockname);
            finish_benchmark(&bencher.output_path, "Queue", records.iter());
        }
    }
}

fn start_benchmark<T>(
    bencher: &Bencher,
    concurrent_queue: impl ConcurrentQueue<T>,
    queue_name: &str,
) -> Vec<Records>
where
    T: Send + Default,
{
    println!("Start benchmark for {}", queue_name);

    let stop_signal = Arc::new(AtomicBool::new(false));
    let lock_ref = Arc::new(concurrent_queue);

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
                    let mut loop_count = 0;
                    let mut num_acquire = 0;
                    let mut aux = 0;

                    let rng = &mut rand::thread_rng();

                    while !stop_signal.load(Ordering::Acquire) {
                        let begin = if stat_response_time {
                            unsafe { __rdtscp(&mut aux) }
                        } else {
                            0
                        };

                        if rng.gen_bool(0.5) {
                            lock_ref.push(T::default());
                        } else {
                            lock_ref.pop();
                        }

                        num_acquire += 1;

                        if stat_response_time {
                            let end = unsafe { __rdtscp(&mut aux) };
                            response_times.push(Some(Duration::from_nanos(end - begin)));
                        }

                        let non_cs_loop = rng.gen_range(1..=8);

                        for _ in 0..non_cs_loop {
                            spin_loop()
                        }

                        loop_count += 1;
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
                        response_times: Some(response_times),
                        locktype: queue_name.to_owned(),
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
    records: impl Iterator<Item = &'a Records> + Clone,
) {
    write_results(output_path, file_name, records.clone());

    for record in records.clone() {
        println!("{}", record.loop_count);
    }

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
