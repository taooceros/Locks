use std::{
    cell::{OnceCell, RefCell},
    fmt::Display,
    hint::{black_box, spin_loop},
    mem::MaybeUninit,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use arrow_ipc::{
    writer::{FileWriter, IpcWriteOptions},
    CompressionType,
};
use itertools::izip;
use libdlock::dlock2::{DLock2, DLock2Delegate};

use crate::{
    benchmark::{
        bencher::Bencher,
        helper::create_plain_writer,
        records::{Record, Records, RecordsBuilder},
    },
    lock_target::{self, DLock2Target},
};

pub fn proportional_counter<'a>(
    bencher: &Bencher,
    file_name: &str,
    targets: impl Iterator<Item = &'a DLock2Target>,
    cs_loop: impl Iterator<Item = usize> + Clone,
    non_cs_loop: impl Iterator<Item = usize> + Clone,
) {
    for target in targets {
        let lock = target.to_locktype(
            0usize,
            Data::Input { data: 0 },
            black_box(|data: &mut usize, input: Data| {
                let data = black_box(data);
                let mut input = black_box(input);

                if let Data::Input {
                    data: mut loop_limit,
                } = input
                {
                    while loop_limit > 0 {
                        *data += 1;
                        loop_limit -= 1;
                    }
                }

                Data::Output {
                    response_time: Duration::from_secs(0),
                    data: *data,
                }
            }),
        );

        if let Some(lock) = lock {
            let records = start_benchmark(bencher, cs_loop.clone(), non_cs_loop.clone(), lock);
            finish_benchmark(&bencher.output_path, file_name, records.iter());
        }
    }
}

pub enum Data {
    Input {
        data: usize,
    },
    Output {
        response_time: Duration,
        data: usize,
    },
}

fn start_benchmark<F>(
    bencher: &Bencher,
    cs_loop: impl Iterator<Item = usize> + Clone,
    non_cs_loop: impl Iterator<Item = usize> + Clone,
    lock_target: impl DLock2<usize, Data, F> + 'static + Display,
) -> Vec<Records>
where
    F: DLock2Delegate<usize, Data>,
{
    println!("Start benchmark for {}", lock_target);

    let stop_signal = Arc::new(AtomicBool::new(false));
    let lock_ref = Arc::new(lock_target);

    let core_ids = core_affinity::get_core_ids().unwrap();
    let core_ids = core_ids.iter().take(bencher.num_thread);

    // println!("{:?}", bencher);

    thread::scope(move |scope| {
        let handles = izip!(cs_loop.cycle(), non_cs_loop.cycle(), core_ids.cycle(),)
            .take(bencher.num_thread)
            .map(|(cs_loop, non_cs_loop, core_id)| {
                let lock_ref = lock_ref.clone();
                let core_id = *core_id;
                let stop_signal = stop_signal.clone();

                scope.spawn(move || {
                    core_affinity::set_for_current(core_id);

                    let stop_signal = black_box(stop_signal);
                    let cs_loop = black_box(cs_loop);
                    let non_cs_loop = black_box(non_cs_loop);

                    let mut loop_count = 0;

                    while !stop_signal.load(Ordering::Acquire) {
                        let data = Data::Input { data: cs_loop };
                        lock_ref.lock(data);
                        loop_count += cs_loop;

                        for _ in 0..non_cs_loop {
                            spin_loop()
                        }
                    }

                    Records {
                        id: 0,
                        cpu_id: core_id.id,
                        thread_num: 0,
                        cpu_num: 0,
                        loop_count: loop_count as u64,
                        num_acquire: 0,
                        cs_length: Default::default(),
                        non_cs_length: None,
                        is_combiner: None,
                        response_times: None,
                        hold_time: Default::default(),
                        combine_time: None,
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
                let option = IpcWriteOptions::try_new(8, false, arrow::ipc::MetadataVersion::V5)
                    .unwrap()
                    .try_with_compression(Some(CompressionType::ZSTD))
                    .expect("Failed to create compression option");

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
