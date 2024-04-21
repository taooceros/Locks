use std::{
    arch::x86_64::__rdtscp,
    hint::black_box,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self},
    time::Duration,
};

use rand::Rng;

use crate::{
    benchmark::{bencher::Bencher, records::*},
    lock_target::DLock2Target,
};

use self::extension::*;

pub mod extension;

pub fn lockfree_queue<'a>(bencher: &Bencher, queues: Vec<LockFreeQueue>) {
    for queue in queues {
        todo!()
    }
}

pub fn benchmark_queue<'a, Q: SequentialQueue<u64> + Send + Sync + 'static>(
    bencher: &Bencher,
    queue: impl Fn() -> Q,
    targets: impl Iterator<Item = &'a DLock2Target>,
) {
    for target in targets {
        let lock = target.to_locktype(
            queue(),
            QueueData::default(),
            move |queue: &mut Q, input: QueueData<u64>| match input {
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
            let queue_name = format!("{}-queue", lock);
            let records = start_benchmark(bencher, lock, &queue_name);
            finish_benchmark(
                &bencher.output_path,
                &queue_name,
                if bencher.stat_response_time {
                    "Queue (latency)"
                } else {
                    "Queue"
                },
                records,
            );
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
                            response_times.push(end - begin);
                        }

                        let non_cs_loop = rng.gen_range(1..=64);

                        for i in 0..non_cs_loop {
                            black_box(i);
                        }

                        loop_count += 1;
                    }

                    Records {
                        id,
                        cpu_id: core_id.id,
                        loop_count,
                        num_acquire,
                        non_cs_length: Some(0),
                        waiter_latency: response_times,
                        locktype: queue_name.to_owned(),
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
    queue_name: &str,
    file_name: &str,
    records: impl AsRef<Vec<Records>>,
) {
    let folder = output_path.join(queue_name);

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
