use std::{
    arch::x86_64::__rdtscp,
    fmt::Display,
    hint::black_box,
    io::Write,
    iter::zip,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, current, ThreadId},
    time::Duration,
};

use bitvec::prelude::*;
use itertools::izip;
use libdlock::dlock2::DLock2;

use crate::benchmark::{
    bencher::Bencher,
    helper::create_plain_writer,
    records::{write_results, Records},
};

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

/// Compute percentiles from a sorted slice of latencies.
/// Returns (p50, p95, p99, p99.9, min, max, mean).
pub fn compute_percentiles(sorted: &[u64]) -> (u64, u64, u64, u64, u64, u64, f64) {
    let n = sorted.len();
    if n == 0 {
        return (0, 0, 0, 0, 0, 0, 0.0);
    }
    let p = |frac: f64| -> u64 {
        let idx = ((n as f64) * frac).ceil() as usize;
        sorted[idx.min(n - 1)]
    };
    let sum: u64 = sorted.iter().sum();
    let mean = sum as f64 / n as f64;
    (
        p(0.50),
        p(0.95),
        p(0.99),
        p(0.999),
        sorted[0],
        sorted[n - 1],
        mean,
    )
}

/// Print percentile summary for a latency distribution.
pub fn print_percentiles(label: &str, sorted: &[u64]) {
    if sorted.is_empty() {
        return;
    }
    let (p50, p95, p99, p999, min, max, mean) = compute_percentiles(sorted);
    println!(
        "  {label} (n={}): mean={:.0} p50={} p95={} p99={} p99.9={} min={} max={}",
        sorted.len(),
        mean,
        p50,
        p95,
        p99,
        p999,
        min,
        max,
    );
}

/// Write CDF data as CSV: each row is (latency_cycles, cumulative_fraction).
/// Downsamples to at most `max_points` rows to keep file sizes reasonable.
pub fn write_cdf_csv(path: &Path, sorted: &[u64], max_points: usize) {
    let n = sorted.len();
    if n == 0 {
        return;
    }
    let mut file = match create_plain_writer(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Warning: failed to create CDF file {}: {e}", path.display());
            return;
        }
    };
    let _ = writeln!(file, "latency_cycles,cumulative_fraction");
    let step = if n > max_points { n / max_points } else { 1 };
    for (i, &val) in sorted.iter().enumerate() {
        if i % step == 0 || i == n - 1 {
            let frac = (i + 1) as f64 / n as f64;
            let _ = writeln!(file, "{},{:.6}", val, frac);
        }
    }
}

/// Print response time percentiles and export CDF CSVs for a set of records.
/// `folder` is the output directory for this lock (e.g. `visualization/output/FC`).
pub fn report_response_times(folder: &Path, file_name: &str, records: &[Records]) {
    let mut all_combiner: Vec<u64> = records
        .iter()
        .flat_map(|r| r.combiner_latency.iter().copied())
        .collect();
    let mut all_waiter: Vec<u64> = records
        .iter()
        .flat_map(|r| r.waiter_latency.iter().copied())
        .collect();

    if all_combiner.is_empty() && all_waiter.is_empty() {
        return;
    }

    all_combiner.sort_unstable();
    all_waiter.sort_unstable();

    let mut all_latencies: Vec<u64> = Vec::with_capacity(all_combiner.len() + all_waiter.len());
    all_latencies.extend_from_slice(&all_combiner);
    all_latencies.extend_from_slice(&all_waiter);
    all_latencies.sort_unstable();

    println!("Response Time (TSC cycles):");
    print_percentiles("all", &all_latencies);
    print_percentiles("combiner", &all_combiner);
    print_percentiles("waiter", &all_waiter);

    // Export CDF CSV files for plotting.
    let cdf_dir = folder.join("cdf");
    let max_cdf_points = 10_000;
    write_cdf_csv(
        &cdf_dir.join(format!("{file_name}_all.csv")),
        &all_latencies,
        max_cdf_points,
    );
    write_cdf_csv(
        &cdf_dir.join(format!("{file_name}_combiner.csv")),
        &all_combiner,
        max_cdf_points,
    );
    write_cdf_csv(
        &cdf_dir.join(format!("{file_name}_waiter.csv")),
        &all_waiter,
        max_cdf_points,
    );
}

pub fn start_benchmark<L>(
    bencher: &Bencher,
    stat_hold_time: bool,
    cs_loop: impl Iterator<Item = u64> + Clone,
    non_cs_loop: impl Iterator<Item = u64> + Clone,
    lock: Arc<L>,
) -> Vec<Records>
where
    L: DLock2<Data> + 'static + Display,
{
    println!(
        "Start benchmark for {} (warmup={}s, duration={}s, trial={})",
        lock,
        bencher.warmup,
        bencher.duration,
        bencher.current_trial(),
    );

    let stop_signal = Arc::new(AtomicBool::new(false));
    // Starts as `false` when warmup > 0 so threads skip accumulation during warmup.
    let warmup_done = Arc::new(AtomicBool::new(bencher.warmup == 0));

    let core_ids = core_affinity::get_core_ids().unwrap();
    let core_ids = core_ids.iter().take(bencher.num_thread);

    thread::scope(move |scope| {
        let handles = izip!(cs_loop.cycle(), non_cs_loop.cycle(), core_ids.cycle(),)
            .take(bencher.num_thread)
            .enumerate()
            .map(|(id, (cs_loop, non_cs_loop, core_id))| {
                let lock_ref = lock.clone();
                let core_id = *core_id;
                let stop_signal = stop_signal.clone();
                let warmup_done = warmup_done.clone();
                let stat_response_time = bencher.stat_response_time;

                scope.spawn(move || {
                    core_affinity::set_for_current(core_id);

                    let stop_signal = stop_signal;
                    let mut latencies = vec![];
                    let mut is_combiners: BitVec<usize, Lsb0> = BitVec::new();
                    let mut loop_count = 0u64;
                    let mut num_acquire = 0u64;
                    let mut aux = 0;

                    let data = Data::Input {
                        data: cs_loop,
                        thread_id: current().id(),
                    };

                    let mut hold_time = 0u64;

                    while !stop_signal.load(Ordering::Acquire) {
                        // Only accumulate stats after warmup has elapsed.
                        let measuring = warmup_done.load(Ordering::Acquire);

                        let begin = if stat_response_time && measuring {
                            unsafe { __rdtscp(&mut aux) }
                        } else {
                            0
                        };

                        let output = lock_ref.lock(data);

                        if let Data::Output {
                            is_combiner,
                            hold_time: current_hold_time,
                            ..
                        } = output
                        {
                            if measuring {
                                num_acquire += 1;

                                if stat_response_time {
                                    let end = unsafe { __rdtscp(&mut aux) };
                                    latencies.push(end - begin);
                                    is_combiners.push(is_combiner);
                                }

                                if stat_hold_time {
                                    hold_time += current_hold_time;
                                }

                                loop_count += cs_loop;
                            }
                        } else {
                            unreachable!();
                        }

                        for i in 0..non_cs_loop {
                            black_box(i);
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
                        cs_length: cs_loop,
                        non_cs_length: Some(non_cs_loop),
                        combiner_latency,
                        waiter_latency,
                        hold_time,
                        combine_time: lock_ref.get_combine_time(),
                        locktype: format!("{}", lock_ref),
                        waiter_type: "".to_string(),
                        ..Records::from_bencher(bencher)
                    }
                })
            })
            .collect::<Vec<_>>();

        // Warmup phase: threads run but stats are not counted.
        if bencher.warmup > 0 {
            thread::sleep(Duration::from_secs(bencher.warmup));
            warmup_done.store(true, Ordering::Release);
        }

        // Measurement phase.
        thread::sleep(Duration::from_secs(bencher.duration));

        stop_signal.store(true, Ordering::Release);

        handles
            .into_iter()
            .map(move |h| h.join().unwrap())
            .collect()
    })
}

pub fn finish_benchmark(
    output_path: &Path,
    file_name: &str,
    lock_name: &str,
    mut records: Vec<Records>,
) {
    let folder = output_path.join(lock_name);

    if !folder.exists() {
        std::fs::create_dir_all(&folder).unwrap();
    }

    // Compute Jain's Fairness Index and per-thread normalized share from hold_time.
    // Only meaningful when hold_time tracking is enabled (i.e. not all zeros).
    let n = records.len();
    let any_hold_time = records.iter().any(|r| r.hold_time > 0);
    if n > 0 && any_hold_time {
        let hold_times: Vec<f64> = records.iter().map(|r| r.hold_time as f64).collect();
        let sum: f64 = hold_times.iter().sum();
        let sum_sq: f64 = hold_times.iter().map(|&x| x * x).sum();

        // JFI = (Σxi)² / (n · Σxi²)
        let jfi = if sum_sq > 0.0 {
            (sum * sum) / (n as f64 * sum_sq)
        } else {
            1.0
        };

        let mean = sum / n as f64;
        let normalized_shares: Vec<f64> = hold_times
            .iter()
            .map(|&x| if mean > 0.0 { x / mean } else { 1.0 })
            .collect();

        for (record, &ns) in records.iter_mut().zip(normalized_shares.iter()) {
            record.jfi = jfi;
            record.normalized_share = ns;
        }

        let shares_str: Vec<String> = normalized_shares
            .iter()
            .map(|&x| format!("{:.4}", x))
            .collect();
        println!("Fairness Metrics:");
        println!("  JFI: {:.4}", jfi);
        println!("  Per-thread normalized share: [{}]", shares_str.join(", "));
    }

    report_response_times(&folder, file_name, &records);

    write_results(&folder, file_name, &records);

    for record in records.iter() {
        println!("{}", record.loop_count);
    }

    let total_loop_count: u64 = records.iter().map(|r| r.loop_count).sum();

    println!("Total loop count: {}", total_loop_count);
}
