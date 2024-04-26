use std::{
    arch::x86_64::__rdtscp,
    hint::black_box,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use core_affinity::CoreId;
use rand::Rng;

use crate::{
    benchmark::{
        bencher::Bencher,
        records::{
            spec::{Latency, Spec, SpecBuilder},
            write_results, Records,
        },
    },
    lock_target::DLock2Target,
};

use self::extension::{
    ConcurrentPriorityQueue, DLock2PriorityQueue, PQData, SequentialPriorityQueue,
};

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
            finish_benchmark(
                &bencher.output_path,
                &lockname,
                if bencher.stat_response_time {
                    "Priority Queue (latency)"
                } else {
                    "Priority Queue"
                },
                records,
            );
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
                    let mut waiter_latency = vec![];
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
                            waiter_latency.push(end - begin);
                        }

                        let non_cs_loop = rng.gen_range(1..=8);

                        for i in 0..non_cs_loop {
                            black_box(i);
                        }

                        loop_count += 1;
                    }

                    Records {
                        spec: Spec::builder()
                            .with_bencher(bencher)
                            .id(id)
                            .cpu_id(core_id.id)
                            .loop_count(loop_count)
                            .num_acquire(num_acquire)
                            .target_name(queue_name.to_string())
                            .build(),
                        latency: Latency {
                            combiner_latency: vec![],
                            waiter_latency,
                        },
                        combiner_stat: Default::default(),
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
    pq_name: &str,
    file_name: &str,
    records: impl AsRef<Vec<Records>>,
) {
    let folder = output_path.join(pq_name);

    if !folder.exists() {
        std::fs::create_dir_all(&folder).unwrap();
    }

    let records = records.as_ref();
    write_results(&folder, file_name, records);

    for record in records.iter() {
        println!("{}", record.spec.loop_count);
    }

    let total_loop_count: u64 = records.iter().map(|r| r.spec.loop_count).sum();

    println!("Total loop count: {}", total_loop_count);
}
