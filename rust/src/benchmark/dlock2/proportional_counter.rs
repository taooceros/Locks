use std::{
    arch::x86_64::__rdtscp,
    cell::{OnceCell, RefCell},
    default,
    fmt::Display,
    hint::{black_box, spin_loop},
    mem::MaybeUninit,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, current, ThreadId},
    time::Duration,
};

use arrow_ipc::{
    writer::{FileWriter, IpcWriteOptions},
    CompressionType,
};
use itertools::izip;
use libdlock::dlock2::{DLock2, DLock2Delegate};
use nix::sys::time;

use crate::{
    benchmark::{
        bencher::Bencher,
        helper::create_plain_writer,
        records::{Records, RecordsBuilder},
    },
    lock_target::DLock2Target,
};

pub fn proportional_counter<'a>(
    bencher: &Bencher,
    file_name: &str,
    targets: impl Iterator<Item = &'a DLock2Target>,
    cs_loop: impl Iterator<Item = usize> + Clone,
    non_cs_loop: impl Iterator<Item = usize> + Clone,
) {
    for target in targets {
        let stat_response_time = bencher.stat_response_time;

        let lock = target.to_locktype(
            0usize,
            Data::default(),
            move |data: &mut usize, input: Data| {
                let data = black_box(data);
                let input = black_box(input);

                let timestamp = unsafe {
                    if stat_response_time {
                        __rdtscp(&mut 0)
                    } else {
                        0
                    }
                };

                if let Data::Input {
                    thread_id,
                    data: loop_limit,
                } = input
                {
                    // it is very important to have black_box here
                    let mut loop_limit = black_box(loop_limit);

                    while loop_limit > 0 {
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
            },
        );

        if let Some(lock) = lock {
            let records = start_benchmark(bencher, cs_loop.clone(), non_cs_loop.clone(), lock);
            finish_benchmark(&bencher.output_path, file_name, records.iter());
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum Data {
    #[default]
    Nothing,
    Input {
        data: usize,
        thread_id: ThreadId,
    },
    Output {
        timestamp: u64,
        is_combiner: bool,
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
            .enumerate()
            .map(|(id, (cs_loop, non_cs_loop, core_id))| {
                let lock_ref = lock_ref.clone();
                let core_id = *core_id;
                let stop_signal = stop_signal.clone();
                let stat_response_time = bencher.stat_response_time;

                scope.spawn(move || {
                    core_affinity::set_for_current(core_id);

                    let stop_signal = black_box(stop_signal);
                    let cs_loop = black_box(cs_loop);
                    let non_cs_loop = black_box(non_cs_loop);
                    let mut response_times = vec![];
                    let mut is_combiners = vec![];
                    let mut loop_count = 0;
                    let mut num_acquire = 0;
                    let mut aux = 0;

                    let data = Data::Input {
                        data: cs_loop,
                        thread_id: current().id(),
                    };

                    while !stop_signal.load(Ordering::Acquire) {
                        let begin = if stat_response_time {
                            unsafe { __rdtscp(&mut aux) }
                        } else {
                            0
                        };

                        let output = lock_ref.lock(data);

                        if let Data::Output { is_combiner, .. } = output {
                            is_combiners.push(is_combiner);
                        } else {
                            panic!("Invalid output");
                        }

                        num_acquire += 1;

                        if stat_response_time {
                            let end = unsafe { __rdtscp(&mut aux) };
                            response_times.push(Some(Duration::from_nanos(end - begin)));
                        }

                        loop_count += cs_loop;

                        for i in 0..non_cs_loop {
                            black_box(i);
                            spin_loop()
                        }
                    }

                    println!("response_time_length: {:?}", &response_times[0..10]);
                    println!("length {}", response_times.len());

                    Records {
                        id,
                        cpu_id: core_id.id,
                        thread_num: bencher.num_thread,
                        cpu_num: bencher.num_cpu,
                        loop_count: loop_count as u64,
                        num_acquire,
                        cs_length: Duration::from_nanos(cs_loop as u64),
                        non_cs_length: Some(Duration::from_nanos(non_cs_loop as u64)),
                        is_combiner: None,
                        response_times: Some(response_times),
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
                    .unwrap();
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
