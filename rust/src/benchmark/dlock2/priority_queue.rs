use std::{
    arch::x86_64::__rdtscp,
    borrow::Borrow,
    cell::{OnceCell, RefCell},
    collections::{BTreeSet, BinaryHeap, LinkedList},
    hint::{black_box, spin_loop},
    ops::Deref,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use arrow_ipc::writer::{FileWriter, IpcWriteOptions};
use crossbeam_skiplist::SkipMap;
use libdlock::dlock2::DLock2;
use rand::Rng;

use crate::{
    benchmark::{
        bencher::Bencher,
        helper::create_plain_writer,
        records::{Records, RecordsBuilder},
    },
    lock_target::DLock2Target,
};

use self::extension::{
    ConcurrentPriorityQueue, DLock2PriorityQueue, PQData, SequentialPriorityQueue,
};

use super::queue::{ConcurrentQueue, QueueData};

mod extension;

fn pq_operation<'a, T>(
    queue: &'a mut impl SequentialPriorityQueue<T>,
    input: PQData<T>,
) -> PQData<T>
where
    T: Send + Default + Ord + Sync + Copy,
{
    match input {
        PQData::Push { data } => {
            queue.push(data);
            PQData::Nothing
        }
        PQData::Pop => {
            let output = queue.pop();
            PQData::PopResult(output)
        }
        PQData::Peek => {
            let output = queue.peek();
            PQData::PeekResult(output.map(|x| *x))
        }
        _ => panic!("Invalid input"),
    }
}

pub fn benchmark_pq<'a, S: SequentialPriorityQueue<u64> + Send + Sync + 'static>(
    bencher: &Bencher,
    sequencial_pq: impl Fn() -> S,
    targets: impl Iterator<Item = &'a DLock2Target>,
    // lock_free_queues: &Vec<LockFreeQueue>,
) {
    for target in targets {
        let lock = target.to_locktype(sequencial_pq(), PQData::<u64>::default(), pq_operation);

        if let Some(lock) = lock {
            let queue = DLock2PriorityQueue::<u64, S, _>::new(lock);

            let lockname = format!("{}-queue", queue.inner);
            let records = start_benchmark(bencher, queue, &lockname);
            finish_benchmark(&bencher.output_path, "FetchAndMultiply", records.iter());
        }
    }
}

fn start_benchmark<'a, T>(
    bencher: &Bencher,
    concurrent_queue: impl ConcurrentPriorityQueue<T> + 'a,
    queue_name: &str,
) -> Vec<Records>
where
    T: Send + Default + Ord + Sync,
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
                let stop_signal = stop_signal.clone();

                scope.spawn(move || {
                    let lock_ref = lock_ref.clone();
                    let core_id = *core_id;
                    let stop_signal = stop_signal.clone();
                    let stat_response_time = bencher.stat_response_time;
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
