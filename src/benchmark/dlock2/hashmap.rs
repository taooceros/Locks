use std::{
    arch::x86_64::__rdtscp,
    collections::HashMap,
    fmt::Display,
    hint::black_box,
    iter::zip,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, current, ThreadId},
    time::Duration,
};

use bitvec::prelude::*;
use libdlock::dlock2::DLock2;
use rand::Rng;

use crate::{
    benchmark::{bencher::Bencher, records::Records},
    lock_target::DLock2Target,
};

use super::counter_common::finish_benchmark;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// 64-byte value — one cache line per entry.
/// 10K entries = ~640 KB (exceeds L1, fits in L2).
pub type Value = [u8; 64];

/// Input/output type for the HashMap delegation lock.
#[derive(Debug, Clone)]
pub enum HashMapOp {
    // Inputs (thread -> combiner)
    Get {
        key: u64,
        thread_id: ThreadId,
    },
    Put {
        key: u64,
        value: Value,
        thread_id: ThreadId,
    },
    Scan {
        count: usize,
        thread_id: ThreadId,
    },

    // Outputs (combiner -> thread)
    GetResult {
        found: bool,
        hold_time: u64,
        is_combiner: bool,
    },
    PutResult {
        hold_time: u64,
        is_combiner: bool,
    },
    ScanResult {
        entries_scanned: usize,
        hold_time: u64,
        is_combiner: bool,
    },

    Nothing,
}

impl Default for HashMapOp {
    fn default() -> Self {
        HashMapOp::Nothing
    }
}

// ---------------------------------------------------------------------------
// Zipfian sampler (YCSB-style rejection-inversion)
// ---------------------------------------------------------------------------

struct ZipfianSampler {
    n: u64,
    alpha: f64,
    zetan: f64,
    eta: f64,
    theta: f64,
}

impl ZipfianSampler {
    fn new(n: u64, theta: f64) -> Self {
        let zetan = Self::zeta(n, theta);
        let zeta2 = Self::zeta(2, theta);
        let alpha = 1.0 / (1.0 - theta);
        let eta = (1.0 - (2.0 / n as f64).powf(1.0 - theta)) / (1.0 - zeta2 / zetan);
        Self {
            n,
            alpha,
            zetan,
            eta,
            theta,
        }
    }

    fn zeta(n: u64, theta: f64) -> f64 {
        (1..=n).map(|i| 1.0 / (i as f64).powf(theta)).sum()
    }

    fn sample(&self, rng: &mut impl Rng) -> u64 {
        let u: f64 = rng.gen();
        let uz = u * self.zetan;
        if uz < 1.0 {
            return 0;
        }
        if uz < 1.0 + 0.5f64.powf(self.theta) {
            return 1;
        }
        let val = (self.n as f64 * (self.eta * u - self.eta + 1.0).powf(self.alpha)) as u64;
        val.min(self.n - 1)
    }
}

// ---------------------------------------------------------------------------
// Pre-population
// ---------------------------------------------------------------------------

fn create_initial_map(num_entries: usize) -> HashMap<u64, Value> {
    let mut map = HashMap::with_capacity(num_entries);
    let mut rng = rand::thread_rng();
    for i in 0..num_entries as u64 {
        let mut value = [0u8; 64];
        rng.fill(&mut value[..]);
        map.insert(i, value);
    }
    map
}

// ---------------------------------------------------------------------------
// Benchmark entry point
// ---------------------------------------------------------------------------

pub fn benchmark_hashmap<'a>(
    bencher: &Bencher,
    file_name: &str,
    targets: impl Iterator<Item = &'a DLock2Target>,
    scan_threads: usize,
    scan_sizes: &[usize],
    num_entries: usize,
    get_ratio: f64,
    zipf_theta: f64,
    stat_hold_time: bool,
) {
    for target in targets {
        let initial_map = create_initial_map(num_entries);

        let lock = target.to_locktype(
            initial_map,
            HashMapOp::default(),
            #[inline(never)]
            move |data: &mut HashMap<u64, Value>, input: HashMapOp| -> HashMapOp {
                match input {
                    HashMapOp::Get { key, thread_id } => {
                        let ts = unsafe {
                            if stat_hold_time {
                                __rdtscp(&mut 0)
                            } else {
                                0
                            }
                        };

                        let found = if let Some(v) = data.get(&key) {
                            black_box(v);
                            true
                        } else {
                            false
                        };

                        let hold_time = if stat_hold_time {
                            let end = unsafe { __rdtscp(&mut 0) };
                            end - ts
                        } else {
                            0
                        };

                        HashMapOp::GetResult {
                            found,
                            hold_time,
                            is_combiner: current().id() == thread_id,
                        }
                    }
                    HashMapOp::Put {
                        key,
                        value,
                        thread_id,
                    } => {
                        let ts = unsafe {
                            if stat_hold_time {
                                __rdtscp(&mut 0)
                            } else {
                                0
                            }
                        };

                        data.insert(key, value);

                        let hold_time = if stat_hold_time {
                            let end = unsafe { __rdtscp(&mut 0) };
                            end - ts
                        } else {
                            0
                        };

                        HashMapOp::PutResult {
                            hold_time,
                            is_combiner: current().id() == thread_id,
                        }
                    }
                    HashMapOp::Scan { count, thread_id } => {
                        let ts = unsafe {
                            if stat_hold_time {
                                __rdtscp(&mut 0)
                            } else {
                                0
                            }
                        };

                        let mut scanned = 0usize;
                        let mut checksum = 0u64;
                        for (k, v) in data.iter() {
                            if scanned >= count {
                                break;
                            }
                            checksum = checksum.wrapping_add(*k);
                            checksum = checksum.wrapping_add(v[0] as u64);
                            scanned += 1;
                        }
                        black_box(checksum);

                        let hold_time = if stat_hold_time {
                            let end = unsafe { __rdtscp(&mut 0) };
                            end - ts
                        } else {
                            0
                        };

                        HashMapOp::ScanResult {
                            entries_scanned: scanned,
                            hold_time,
                            is_combiner: current().id() == thread_id,
                        }
                    }
                    _ => panic!("Invalid hashmap input"),
                }
            },
        );

        if let Some(lock) = lock {
            let lock = Arc::new(lock);

            let records = start_hashmap_benchmark(
                bencher,
                lock.clone(),
                num_entries,
                scan_threads,
                scan_sizes,
                get_ratio,
                zipf_theta,
            );

            finish_benchmark(&bencher.output_path, file_name, &lock.to_string(), records);
        }
    }
}

// ---------------------------------------------------------------------------
// Custom benchmark loop with heterogeneous thread roles
// ---------------------------------------------------------------------------

fn start_hashmap_benchmark<L>(
    bencher: &Bencher,
    lock: Arc<L>,
    num_entries: usize,
    scan_threads: usize,
    scan_sizes: &[usize],
    get_ratio: f64,
    zipf_theta: f64,
) -> Vec<Records>
where
    L: DLock2<HashMapOp> + 'static + Display,
{
    println!(
        "Start benchmark for {} (warmup={}s, duration={}s, trial={}, scan_threads={}, scan_sizes={:?}, entries={}, get_ratio={}, zipf_theta={})",
        lock,
        bencher.warmup,
        bencher.duration,
        bencher.current_trial(),
        scan_threads,
        scan_sizes,
        num_entries,
        get_ratio,
        zipf_theta,
    );

    let stop_signal = Arc::new(AtomicBool::new(false));
    let warmup_done = Arc::new(AtomicBool::new(bencher.warmup == 0));

    let core_ids = core_affinity::get_core_ids().unwrap();
    let core_ids: Vec<_> = core_ids.iter().take(bencher.num_thread).collect();

    // Clone scan_sizes into a Vec so we can move it into thread::scope.
    let scan_sizes: Vec<usize> = scan_sizes.to_vec();

    thread::scope(|scope| {
        let handles: Vec<_> = (0..bencher.num_thread)
            .map(|id| {
                let lock = lock.clone();
                let stop = stop_signal.clone();
                let warmup = warmup_done.clone();
                let core_id = *core_ids[id % core_ids.len()];
                let stat_response_time = bencher.stat_response_time;

                // Last `scan_threads` threads are scanners.
                let is_scanner = scan_threads > 0 && id >= bencher.num_thread - scan_threads;
                let scan_size = if is_scanner && !scan_sizes.is_empty() {
                    scan_sizes[id % scan_sizes.len()]
                } else {
                    0
                };

                scope.spawn(move || {
                    core_affinity::set_for_current(core_id);

                    let mut rng = rand::thread_rng();
                    let zipf = ZipfianSampler::new(num_entries as u64, zipf_theta);
                    let thread_id = current().id();

                    let mut latencies = vec![];
                    let mut is_combiners: BitVec<usize, Lsb0> = BitVec::new();
                    let mut loop_count = 0u64;
                    let mut num_acquire = 0u64;
                    let mut hold_time_total = 0u64;
                    let mut aux = 0u32;

                    while !stop.load(Ordering::Acquire) {
                        let measuring = warmup.load(Ordering::Acquire);

                        // Choose operation based on thread role.
                        let op = if is_scanner {
                            HashMapOp::Scan {
                                count: scan_size,
                                thread_id,
                            }
                        } else if rng.gen_bool(get_ratio) {
                            HashMapOp::Get {
                                key: zipf.sample(&mut rng),
                                thread_id,
                            }
                        } else {
                            let mut value = [0u8; 64];
                            rng.fill(&mut value[..]);
                            HashMapOp::Put {
                                key: zipf.sample(&mut rng),
                                value,
                                thread_id,
                            }
                        };

                        let begin = if stat_response_time && measuring {
                            unsafe { __rdtscp(&mut aux) }
                        } else {
                            0
                        };

                        let output = lock.lock(op);

                        let (ht, is_comb) = match &output {
                            HashMapOp::GetResult {
                                hold_time,
                                is_combiner,
                                ..
                            } => (*hold_time, *is_combiner),
                            HashMapOp::PutResult {
                                hold_time,
                                is_combiner,
                            } => (*hold_time, *is_combiner),
                            HashMapOp::ScanResult {
                                hold_time,
                                is_combiner,
                                ..
                            } => (*hold_time, *is_combiner),
                            _ => panic!("Invalid hashmap output"),
                        };

                        if measuring {
                            num_acquire += 1;
                            loop_count += 1;
                            hold_time_total += ht;

                            if stat_response_time {
                                let end = unsafe { __rdtscp(&mut aux) };
                                latencies.push(end - begin);
                                is_combiners.push(is_comb);
                            }
                        }

                        // Small non-CS work for lookup threads.
                        if !is_scanner {
                            for i in 0..8u64 {
                                black_box(i);
                            }
                        }
                    }

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

                    Records {
                        id,
                        cpu_id: core_id.id,
                        loop_count,
                        num_acquire,
                        cs_length: 0,
                        non_cs_length: Some(0),
                        combiner_latency,
                        waiter_latency,
                        hold_time: hold_time_total,
                        combine_time: lock.get_combine_time(),
                        locktype: format!("{}", lock),
                        waiter_type: "".to_string(),
                        ..Records::from_bencher(bencher)
                    }
                })
            })
            .collect();

        // Warmup phase.
        if bencher.warmup > 0 {
            thread::sleep(Duration::from_secs(bencher.warmup));
            warmup_done.store(true, Ordering::Release);
        }

        // Measurement phase.
        thread::sleep(Duration::from_secs(bencher.duration));
        stop_signal.store(true, Ordering::Release);

        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}
