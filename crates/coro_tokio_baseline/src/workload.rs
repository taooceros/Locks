//! `coro_delegation`'s workload (`crates/coro_delegation/src/workload.rs`,
//! RESEARCH.md "Workload") on tokio's multi-thread runtime.
//!
//! Identical to the original: shared `BTreeMap<u64, u64>`; a critical section
//! inserts a key from a per-client xorshift stream (same seed derivation) and
//! spins `class_cost` cycles; even client ids light, odd heavy
//! (`heavy_ratio` x light); each client loops `run(cs).await` then spends
//! `parallel_work` cycles ([`ParallelMode`]: spun in the same poll, or after
//! one `yield_now().await`); bystanders loop `spin(bystander_work)` +
//! `yield_now().await`; warm-up -> measure -> stop, an op is attributed to the
//! phase in which it started; service cycles are measured inside the critical
//! section (insert + spin). Differences, all forced by the runtime:
//!
//! - Scheduler: tokio multi-thread (`worker_threads = W`, worker thread `i`
//!   by start order pinned to logical CPU `i`), default configuration (LIFO
//!   slot on, coop budget 128, `event_interval` 61). A worker steals (half of
//!   a random peer's run queue, never its LIFO slot) whenever its own LIFO
//!   slot and run queue are empty; there is no periodic balancing steal.
//! - `yield_now()` puts the task in the worker's defer list, not its run
//!   queue; deferred tasks are woken only at the next maintenance tick
//!   (every 61 polls) or when the worker has nothing else to run and has
//!   failed to steal. A worker whose only runnable tasks are yielded
//!   bystanders therefore counts as idle for stealing.
//! - Initial placement: every task is spawned from the (non-worker) main
//!   thread, i.e. through tokio's injection queue; tokio has no `spawn_on`.
//! - Bystander latency is measured by the task itself: from the `yield_now()`
//!   call (or the spawn) to the start of the next poll, so it includes the
//!   deferral above.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::ValueEnum;
use serde::Serialize;
use tokio::task::yield_now;

use crate::stats::{cycles, jain, spin_cycles, Histogram, LatencySummary, XorShift64};

pub type Shared = BTreeMap<u64, u64>;

const PHASE_WARMUP: u8 = 0;
const PHASE_MEASURE: u8 = 1;
const PHASE_STOP: u8 = 2;

// ---------------------------------------------------------------------------
// Locks
// ---------------------------------------------------------------------------

/// A lock around `Shared`, exercised like `coro_delegation`'s
/// `LockClient::run`: the future completes after `f` ran with exclusive
/// access and the lock was released. `f` never awaits.
pub trait BenchLock: Send + Sync + 'static {
    fn new(data: Shared) -> Self;

    fn run<R, F>(&self, f: F) -> impl Future<Output = R> + Send
    where
        R: Send,
        F: FnOnce(&mut Shared) -> R + Send;
}

pub struct TokioMutex(tokio::sync::Mutex<Shared>);

impl BenchLock for TokioMutex {
    fn new(data: Shared) -> Self {
        TokioMutex(tokio::sync::Mutex::new(data))
    }

    fn run<R, F>(&self, f: F) -> impl Future<Output = R> + Send
    where
        R: Send,
        F: FnOnce(&mut Shared) -> R + Send,
    {
        async move { f(&mut *self.0.lock().await) }
    }
}

pub struct AsyncLockMutex(async_lock::Mutex<Shared>);

impl BenchLock for AsyncLockMutex {
    fn new(data: Shared) -> Self {
        AsyncLockMutex(async_lock::Mutex::new(data))
    }

    fn run<R, F>(&self, f: F) -> impl Future<Output = R> + Send
    where
        R: Send,
        F: FnOnce(&mut Shared) -> R + Send,
    {
        async move { f(&mut *self.0.lock().await) }
    }
}

/// Blocking lock inside async code: a waiting client blocks its worker
/// thread. The guard never lives across an await, so the future is `Send`.
pub struct StdMutex(std::sync::Mutex<Shared>);

impl BenchLock for StdMutex {
    fn new(data: Shared) -> Self {
        StdMutex(std::sync::Mutex::new(data))
    }

    fn run<R, F>(&self, f: F) -> impl Future<Output = R> + Send
    where
        R: Send,
        F: FnOnce(&mut Shared) -> R + Send,
    {
        async move { f(&mut *self.0.lock().expect("std mutex poisoned")) }
    }
}

pub struct ParkingLotMutex(parking_lot::Mutex<Shared>);

impl BenchLock for ParkingLotMutex {
    fn new(data: Shared) -> Self {
        ParkingLotMutex(parking_lot::Mutex::new(data))
    }

    fn run<R, F>(&self, f: F) -> impl Future<Output = R> + Send
    where
        R: Send,
        F: FnOnce(&mut Shared) -> R + Send,
    {
        async move { f(&mut *self.0.lock()) }
    }
}

/// `--lock` values; the kebab-case name is the `lock` field of the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum LockId {
    /// `tokio::sync::Mutex` (FIFO semaphore handoff; acquire consumes coop budget)
    TokioMutex,
    /// `async_lock::Mutex` (barging; fair handoff after 0.5 ms of starvation)
    AsyncLock,
    /// `std::sync::Mutex` held and waited on inside the task (blocks the worker)
    StdMutex,
    /// `parking_lot::Mutex` held and waited on inside the task (blocks the worker)
    ParkingLot,
    /// `tokio::sync::Mutex`, every client loop wrapped in
    /// `tokio::task::unconstrained` (acquires never forced to yield by the
    /// coop budget)
    TokioMutexUnconstrained,
}

impl LockId {
    pub const ALL: [LockId; 5] = [
        LockId::TokioMutex,
        LockId::AsyncLock,
        LockId::StdMutex,
        LockId::ParkingLot,
        LockId::TokioMutexUnconstrained,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LockId::TokioMutex => "tokio-mutex",
            LockId::AsyncLock => "async-lock",
            LockId::StdMutex => "std-mutex",
            LockId::ParkingLot => "parking-lot",
            LockId::TokioMutexUnconstrained => "tokio-mutex-unconstrained",
        }
    }

    fn run(self, cfg: &Config) -> RawRun {
        match self {
            LockId::TokioMutex => run_raw::<TokioMutex>(cfg, false),
            LockId::AsyncLock => run_raw::<AsyncLockMutex>(cfg, false),
            LockId::StdMutex => run_raw::<StdMutex>(cfg, false),
            LockId::ParkingLot => run_raw::<ParkingLotMutex>(cfg, false),
            LockId::TokioMutexUnconstrained => run_raw::<TokioMutex>(cfg, true),
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration and results (field names follow coro_delegation's `Report`)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct Config {
    pub workers: usize,
    pub clients: usize,
    pub bystanders: usize,
    pub heavy_ratio: u64,
    pub light_cs_cycles: u64,
    pub parallel_work_cycles: u64,
    pub bystander_work_cycles: u64,
    pub key_space: u64,
    pub duration_ms: u64,
    pub warmup_ms: u64,
    pub seed: u64,
    /// Sanity mode: every client inserts unique keys so `len == ops`.
    pub unique_keys: bool,
    /// How a client spends its parallel work after each op (as
    /// `coro_delegation`'s `config.parallel_mode`).
    pub parallel_mode: ParallelMode,
}

/// How a client spends `parallel_work_cycles` after an op; same semantics
/// as `coro_delegation::workload::ParallelMode` (REVIEW-2026-09-30 I1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ParallelMode {
    /// Spin synchronously in the poll that finished the op (the original
    /// harness; worst case for tokio's LIFO slot).
    #[default]
    Spin,
    /// `tokio::task::yield_now().await`, then spin in a later poll.
    Yield,
}

impl ParallelMode {
    pub fn label(self) -> &'static str {
        match self {
            ParallelMode::Spin => "spin",
            ParallelMode::Yield => "yield",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Light,
    Heavy,
}

impl Class {
    pub fn of(id: usize) -> Class {
        if id % 2 == 0 {
            Class::Light
        } else {
            Class::Heavy
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Class::Light => "light",
            Class::Heavy => "heavy",
        }
    }
}

pub struct ClientResult {
    pub id: usize,
    pub class: Class,
    pub ops: u64,
    pub service_cycles: u64,
    pub run_latency: Histogram,
    /// All ops, including warm-up (sanity mode).
    pub total_ops: u64,
}

#[derive(Clone, Serialize)]
pub struct ClientReport {
    pub id: usize,
    pub class: Class,
    pub ops: u64,
    /// Harness-measured cycles inside the critical section (insert + spin).
    pub service_cycles: u64,
    /// `run(cs).await` latency (submission to result, lock released).
    pub run_latency: LatencySummary,
}

#[derive(Clone, Serialize)]
pub struct ClassReport {
    pub clients: usize,
    pub ops: u64,
    pub service_cycles: u64,
    pub run_latency: LatencySummary,
}

/// Per tokio worker (tokio's worker index, not the pinning order), delta
/// over the measurement window of the stable `worker_park_count` metric
/// (incremented and published by tokio at each park, so it is exact).
/// tokio's busy-duration metric is published only at park/maintenance, i.e.
/// never by a worker captured by a non-yielding task, so it is not recorded.
#[derive(Clone, Serialize)]
pub struct TokioWorkerReport {
    pub worker: usize,
    /// Times the worker found no local work, failed to steal and parked
    /// with an empty defer list. Not a steal count: a worker holding only
    /// yielded (deferred) tasks steals and then polls its driver with a zero
    /// timeout without being counted here.
    pub parks: u64,
}

#[derive(Clone, Serialize)]
pub struct Report {
    pub runtime: &'static str,
    pub lock: String,
    pub config: Config,
    pub timestamp: String,
    pub tsc_hz: f64,
    pub measured_secs: f64,
    pub total_ops: u64,
    pub throughput_ops_per_s: f64,
    /// Jain over per-client `service_cycles`.
    pub service_jain: Option<f64>,
    pub clients: Vec<ClientReport>,
    pub classes: BTreeMap<String, ClassReport>,
    /// Clients with zero ops in the window.
    pub starved_clients: usize,
    /// Bystander wait from `yield_now()` (or spawn) to the next poll, merged
    /// over bystanders; sampled at polls during the window and the drain.
    pub bystander_latency: LatencySummary,
    /// Bystander samples >= half the window: tasks that never ran during the
    /// window and were polled only at the drain.
    pub starved_bystanders: u64,
    /// Client loops wrapped in `tokio::task::unconstrained`.
    pub coop_unconstrained_clients: bool,
    /// Logical CPUs the worker threads were pinned to (sorted).
    pub pinned_cpus: Vec<usize>,
    /// Threads the runtime started (`on_thread_start` calls); = workers
    /// unless the blocking pool was used.
    pub runtime_threads: usize,
    pub tokio_workers: Vec<TokioWorkerReport>,
}

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

async fn client_task<L: BenchLock>(
    lock: Arc<L>,
    id: usize,
    cfg: Config,
    phase: Arc<AtomicU8>,
) -> ClientResult {
    let class = Class::of(id);
    let cost = match class {
        Class::Light => cfg.light_cs_cycles,
        Class::Heavy => cfg.light_cs_cycles * cfg.heavy_ratio,
    };
    // Same stream as coro_delegation (its plain `*` wraps in release builds).
    let mut rng = XorShift64(cfg.seed ^ (id as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let mut res = ClientResult {
        id,
        class,
        ops: 0,
        service_cycles: 0,
        run_latency: Histogram::new(),
        total_ops: 0,
    };
    let mut counter: u64 = 0;
    loop {
        let ph = phase.load(Ordering::Relaxed);
        if ph == PHASE_STOP {
            break;
        }
        let key = if cfg.unique_keys {
            ((id as u64) << 40) | counter
        } else {
            rng.next() % cfg.key_space
        };
        counter += 1;
        let t0 = cycles();
        let dt = lock
            .run(move |m: &mut Shared| {
                let s = cycles();
                m.insert(key, s);
                spin_cycles(cost);
                cycles().wrapping_sub(s)
            })
            .await;
        res.total_ops += 1;
        if ph == PHASE_MEASURE {
            res.ops += 1;
            res.service_cycles += dt;
            res.run_latency.record(cycles().wrapping_sub(t0));
        }
        if cfg.parallel_mode == ParallelMode::Yield {
            yield_now().await;
        }
        spin_cycles(cfg.parallel_work_cycles);
    }
    res
}

/// Loops `spin(work)` + `yield_now()`; records the wait from `scheduled`
/// (spawn, then each yield) to the start of every poll while the phase is
/// not warm-up, like coro_delegation's executor-side recording gate.
async fn bystander_task(work: u64, phase: Arc<AtomicU8>, spawned_at: u64) -> Histogram {
    let mut lat = Histogram::new();
    let mut scheduled = spawned_at;
    loop {
        let polled = cycles();
        let ph = phase.load(Ordering::Relaxed);
        if ph != PHASE_WARMUP {
            lat.record(polled.wrapping_sub(scheduled));
        }
        if ph == PHASE_STOP {
            break;
        }
        spin_cycles(work);
        scheduled = cycles();
        yield_now().await;
    }
    lat
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Everything a run produces before it is folded into a `Report`.
pub struct RawRun {
    pub clients: Vec<ClientResult>,
    pub bystanders: Vec<Histogram>,
    pub measured_secs: f64,
    /// `len()` of the shared map after all clients finished.
    pub final_len: usize,
    pub unconstrained: bool,
    pub pinned_cpus: Vec<usize>,
    pub runtime_threads: usize,
    pub tokio_workers: Vec<TokioWorkerReport>,
}

/// Multi-thread runtime with `workers` threads; the i-th thread to start is
/// pinned to logical CPU i (one per physical core on the study machine).
/// Returns the runtime, the successfully pinned CPUs and the thread counter.
fn build_runtime(
    workers: usize,
) -> (
    tokio::runtime::Runtime,
    Arc<Mutex<Vec<usize>>>,
    Arc<AtomicUsize>,
) {
    let pinned = Arc::new(Mutex::new(Vec::with_capacity(workers)));
    let started = Arc::new(AtomicUsize::new(0));
    let rt = {
        let pinned = Arc::clone(&pinned);
        let started = Arc::clone(&started);
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .on_thread_start(move || {
                let i = started.fetch_add(1, Ordering::SeqCst);
                if i < workers && core_affinity::set_for_current(core_affinity::CoreId { id: i }) {
                    pinned.lock().expect("pin list poisoned").push(i);
                }
            })
            .build()
            .expect("build tokio runtime")
    };
    (rt, pinned, started)
}

fn park_counts(m: &tokio::runtime::RuntimeMetrics) -> Vec<u64> {
    (0..m.num_workers())
        .map(|w| m.worker_park_count(w))
        .collect()
}

/// Run one configuration on a fresh runtime and a fresh `L`.
pub fn run_raw<L: BenchLock>(cfg: &Config, unconstrained: bool) -> RawRun {
    let (rt, pinned, started) = build_runtime(cfg.workers);
    let lock = Arc::new(L::new(Shared::new()));
    let phase = Arc::new(AtomicU8::new(if cfg.warmup_ms == 0 {
        PHASE_MEASURE
    } else {
        PHASE_WARMUP
    }));

    let client_tasks: Vec<_> = (0..cfg.clients)
        .map(|id| {
            let fut = client_task(Arc::clone(&lock), id, cfg.clone(), Arc::clone(&phase));
            if unconstrained {
                rt.spawn(tokio::task::unconstrained(fut))
            } else {
                rt.spawn(fut)
            }
        })
        .collect();
    let bystander_tasks: Vec<_> = (0..cfg.bystanders)
        .map(|_| {
            rt.spawn(bystander_task(
                cfg.bystander_work_cycles,
                Arc::clone(&phase),
                cycles(),
            ))
        })
        .collect();

    if cfg.warmup_ms > 0 {
        std::thread::sleep(Duration::from_millis(cfg.warmup_ms));
    }
    let metrics = rt.metrics();
    let before = park_counts(&metrics);
    phase.store(PHASE_MEASURE, Ordering::SeqCst);
    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(cfg.duration_ms));
    phase.store(PHASE_STOP, Ordering::SeqCst);
    let measured_secs = start.elapsed().as_secs_f64();
    let after = park_counts(&metrics);

    let (clients, bystanders) = rt.block_on(async {
        let mut clients = Vec::with_capacity(client_tasks.len());
        for t in client_tasks {
            clients.push(t.await.expect("client task panicked"));
        }
        let mut bystanders = Vec::with_capacity(bystander_tasks.len());
        for t in bystander_tasks {
            bystanders.push(t.await.expect("bystander task panicked"));
        }
        (clients, bystanders)
    });
    let final_len = rt.block_on(lock.run(|m: &mut Shared| m.len()));
    drop(rt);

    let tokio_workers = before
        .iter()
        .zip(&after)
        .enumerate()
        .map(|(worker, (b, a))| TokioWorkerReport {
            worker,
            parks: a - b,
        })
        .collect();
    let mut pinned_cpus = pinned.lock().expect("pin list poisoned").clone();
    pinned_cpus.sort_unstable();
    RawRun {
        clients,
        bystanders,
        measured_secs,
        final_len,
        unconstrained,
        pinned_cpus,
        runtime_threads: started.load(Ordering::SeqCst),
        tokio_workers,
    }
}

pub fn make_report(lock: LockId, cfg: &Config, raw: &RawRun, tsc_hz: f64) -> Report {
    let mut clients: Vec<ClientReport> = raw
        .clients
        .iter()
        .map(|c| ClientReport {
            id: c.id,
            class: c.class,
            ops: c.ops,
            service_cycles: c.service_cycles,
            run_latency: c.run_latency.summary(),
        })
        .collect();
    clients.sort_by_key(|c| c.id);

    let mut classes = BTreeMap::new();
    for class in [Class::Light, Class::Heavy] {
        let mut hist = Histogram::new();
        let (mut n, mut ops, mut svc) = (0usize, 0u64, 0u64);
        for c in raw.clients.iter().filter(|c| c.class == class) {
            n += 1;
            ops += c.ops;
            svc += c.service_cycles;
            hist.merge(&c.run_latency);
        }
        classes.insert(
            class.label().to_string(),
            ClassReport {
                clients: n,
                ops,
                service_cycles: svc,
                run_latency: hist.summary(),
            },
        );
    }

    let mut bystander = Histogram::new();
    for b in &raw.bystanders {
        bystander.merge(b);
    }
    let censor_threshold = (0.5 * raw.measured_secs * tsc_hz) as u64;
    let total_ops: u64 = clients.iter().map(|c| c.ops).sum();

    Report {
        runtime: "tokio",
        lock: lock.label().to_string(),
        config: cfg.clone(),
        timestamp: timestamp(),
        tsc_hz,
        measured_secs: raw.measured_secs,
        total_ops,
        throughput_ops_per_s: total_ops as f64 / raw.measured_secs,
        service_jain: jain(clients.iter().map(|c| c.service_cycles as f64)),
        starved_clients: clients.iter().filter(|c| c.ops == 0).count(),
        clients,
        classes,
        bystander_latency: bystander.summary(),
        starved_bystanders: bystander.count_at_least(censor_threshold),
        coop_unconstrained_clients: raw.unconstrained,
        pinned_cpus: raw.pinned_cpus.clone(),
        runtime_threads: raw.runtime_threads,
        tokio_workers: raw.tokio_workers.clone(),
    }
}

pub fn run_benchmark(lock: LockId, cfg: &Config, tsc_hz: f64) -> Report {
    let raw = lock.run(cfg);
    make_report(lock, cfg, &raw, tsc_hz)
}

#[derive(Clone, Copy, Debug)]
pub struct SanityOutcome {
    pub total_ops: u64,
    pub final_len: usize,
}

/// Mutual-exclusion check: unique keys per client, so the map length must
/// equal the number of completed inserts (warm-up included).
pub fn sanity(lock: LockId, cfg: &Config) -> Result<SanityOutcome, String> {
    let cfg = Config {
        unique_keys: true,
        ..cfg.clone()
    };
    let raw = lock.run(&cfg);
    let total_ops: u64 = raw.clients.iter().map(|c| c.total_ops).sum();
    if raw.final_len as u64 == total_ops {
        Ok(SanityOutcome {
            total_ops,
            final_len: raw.final_len,
        })
    } else {
        Err(format!(
            "map len {} != total ops {} (mutual exclusion violated)",
            raw.final_len, total_ops
        ))
    }
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}
