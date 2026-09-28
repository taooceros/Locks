//! `tokio-bench`: `coro_delegation`'s workload on tokio's multi-thread
//! runtime with off-the-shelf locks, as the cross-runtime baseline for
//! `coro-bench`. One run per invocation (JSON to `--out`), or `--sanity`
//! (mutual-exclusion check over every lock). Flags and defaults mirror
//! `coro-bench`; the report's field names follow its `Report` plus
//! `runtime: "tokio"`.
//!
//! ```text
//! tokio-bench --lock tokio-mutex --workers 8 --clients 64 --heavy-ratio 8 \
//!             --duration-ms 2000 --out results/tokio-tokio-mutex-w8-h8-sus-r1.json
//! tokio-bench --lock std-mutex --workers 16 --clients 16 --parallel-work-cycles 32000 ...
//! tokio-bench --sanity
//! ```

mod stats;
mod workload;

use std::path::PathBuf;

use clap::Parser;

use workload::{Config, LockId, Report};

#[derive(Parser, Debug)]
#[command(
    name = "tokio-bench",
    about = "coro_delegation's workload on tokio with off-the-shelf locks"
)]
struct Cli {
    /// Lock variant
    #[arg(long, value_enum, default_value_t = LockId::TokioMutex)]
    lock: LockId,
    /// tokio worker threads (thread i pinned to logical CPU i)
    #[arg(long, default_value_t = 8)]
    workers: usize,
    /// Client tasks (half light, half heavy)
    #[arg(long, default_value_t = 64)]
    clients: usize,
    /// Bystander tasks (default: = workers)
    #[arg(long)]
    bystanders: Option<usize>,
    /// Heavy critical-section cost as a multiple of light
    #[arg(long, default_value_t = 8)]
    heavy_ratio: u64,
    /// Light critical-section spin, TSC cycles (plus one BTreeMap insert)
    #[arg(long, default_value_t = 1000)]
    light_cs_cycles: u64,
    /// Parallel work between requests, TSC cycles (default: 4 x light CS)
    #[arg(long)]
    parallel_work_cycles: Option<u64>,
    /// Bystander work per poll, TSC cycles (default: = light CS)
    #[arg(long)]
    bystander_work_cycles: Option<u64>,
    /// Random key space of the BTreeMap inserts
    #[arg(long, default_value_t = 65_536)]
    key_space: u64,
    /// Measurement window after warm-up, milliseconds
    #[arg(long, default_value_t = 2000)]
    duration_ms: u64,
    /// Warm-up, milliseconds (not recorded)
    #[arg(long, default_value_t = 200)]
    warmup_ms: u64,
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// Output JSON path (stdout if omitted)
    #[arg(long)]
    out: Option<PathBuf>,
    /// Mutual-exclusion check: every lock, 8 workers, 500 ms
    #[arg(long, default_value_t = false)]
    sanity: bool,
}

impl Cli {
    fn config(&self) -> Config {
        Config {
            workers: self.workers,
            clients: self.clients,
            bystanders: self.bystanders.unwrap_or(self.workers),
            heavy_ratio: self.heavy_ratio,
            light_cs_cycles: self.light_cs_cycles,
            parallel_work_cycles: self
                .parallel_work_cycles
                .unwrap_or(4 * self.light_cs_cycles),
            bystander_work_cycles: self.bystander_work_cycles.unwrap_or(self.light_cs_cycles),
            key_space: self.key_space.max(1),
            duration_ms: self.duration_ms,
            warmup_ms: self.warmup_ms,
            seed: self.seed,
            unique_keys: false,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    if cli.workers == 0 {
        eprintln!("--workers must be >= 1");
        std::process::exit(2);
    }
    let tsc_hz = stats::estimate_tsc_hz();

    if cli.sanity {
        let cfg = Config {
            workers: 8,
            duration_ms: 500,
            warmup_ms: 0,
            unique_keys: true,
            ..cli.config()
        };
        let mut failed = false;
        for id in LockId::ALL {
            match workload::sanity(id, &cfg) {
                Ok(o) => eprintln!(
                    "sanity {:<25} PASS  ops={} len={}",
                    id.label(),
                    o.total_ops,
                    o.final_len
                ),
                Err(e) => {
                    failed = true;
                    eprintln!("sanity {:<25} FAIL  {e}", id.label());
                }
            }
        }
        if failed {
            std::process::exit(1);
        }
        return;
    }

    let cfg = cli.config();
    let report = workload::run_benchmark(cli.lock, &cfg, tsc_hz);
    if report.pinned_cpus.len() != cfg.workers {
        eprintln!(
            "warning: pinned {} of {} workers ({:?})",
            report.pinned_cpus.len(),
            cfg.workers,
            report.pinned_cpus
        );
    }
    print_summary(&report);
    let json = serde_json::to_string_pretty(&report).expect("serialize report");
    match &cli.out {
        Some(path) => {
            if let Some(dir) = path.parent() {
                if !dir.as_os_str().is_empty() {
                    std::fs::create_dir_all(dir).expect("create output directory");
                }
            }
            std::fs::write(path, json).expect("write report");
            eprintln!("wrote {}", path.display());
        }
        None => println!("{json}"),
    }
}

fn print_summary(r: &Report) {
    let us = |c: u64| c as f64 / r.tsc_hz * 1e6;
    eprintln!(
        "{} (tokio) workers={} clients={} heavy_ratio={} measured={:.3}s ops={} throughput={:.0} ops/s",
        r.lock,
        r.config.workers,
        r.config.clients,
        r.config.heavy_ratio,
        r.measured_secs,
        r.total_ops,
        r.throughput_ops_per_s
    );
    eprintln!(
        "  service_jain={} starved_clients={} starved_bystanders={} bystander p50={:.1}us p99={:.1}us pinned={:?} threads={}",
        r.service_jain
            .map_or("null".to_string(), |x| format!("{x:.4}")),
        r.starved_clients,
        r.starved_bystanders,
        us(r.bystander_latency.p50),
        us(r.bystander_latency.p99),
        r.pinned_cpus,
        r.runtime_threads
    );
    for (name, c) in &r.classes {
        eprintln!(
            "  class {:<5} clients={} ops={} run p50={:.1}us p99={:.1}us",
            name,
            c.clients,
            c.ops,
            us(c.run_latency.p50),
            us(c.run_latency.p99)
        );
    }
    let parks: Vec<u64> = r.tokio_workers.iter().map(|w| w.parks).collect();
    eprintln!("  worker parks={parks:?}");
}
