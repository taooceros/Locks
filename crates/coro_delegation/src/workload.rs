//! Benchmark workload (see `RESEARCH.md`, "Workload").
//!
//! - Shared structure `BTreeMap<u64, u64>`; a critical section inserts a
//!   random key (bounded key space so the map stops growing early) and spins
//!   `class_cost` cycles. Heavy class cost = `heavy_ratio` x light.
//! - `clients` client tasks, half light, half heavy (even ids light); each
//!   loops `run(cs).await` (closure locks) or `lock().await; cs;
//!   unlock().await` (coroutine-style `co-*` locks, same body), then its
//!   parallel work ([`ParallelMode`]: spin in the same poll, or
//!   `yield_now()` first).
//! - `bystanders` tasks loop `spin_cycles(bystander_work)` then `yield_now()`;
//!   the executor records their schedule->poll latency.
//! - Phases: warm-up (not recorded) -> measure (recorded) -> stop. An op is
//!   attributed to the phase in which it started.
//!
//! Service cycles are measured by the harness inside the closure (`cycles()`
//! around insert + spin), independent of the lock's own `usage()` accounting,
//! so the service Jain index is comparable across variants.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::executor::{yield_now, Executor};
use crate::lock::{cycles, AsyncGuard, AsyncMutex, CoLock, DelegationLock, LockClient};
use crate::locks::co_mutex::{self, HandleStats};
use crate::locks::{ces, dispatch, fc};
use crate::stats::{self, Histogram, LatencySummary, TaskKind, WorkerStats};

/// Harness-side access to lock-specific client counters that are not part
/// of the `LockClient` trait. Implemented for every concrete client type.
pub trait ClientExtras {
    /// Times this client's task yielded after acting as combiner
    /// (`fc`/`fcpq` cooperative yield); 0 for other locks.
    fn combiner_yields(&self) -> u64 {
        0
    }
}

impl<T: Send + 'static> ClientExtras for dispatch::DispatchClient<T> {}
impl<T: Send + 'static> ClientExtras for crate::locks::dispatch_pq::DispatchPqClient<T> {}
impl<T: Send + 'static> ClientExtras for ces::CesClient<T> {}
impl<T: Send + 'static, P: fc::Policy<T>> ClientExtras for fc::Client<T, P> {
    fn combiner_yields(&self) -> u64 {
        fc::Client::combiner_yields(self)
    }
}
impl<T: Send + 'static> ClientExtras for crate::locks::actor::ActorClient<T> {}
impl<T: Send + 'static> ClientExtras for crate::locks::cfl::CflClient<T> {}

pub type Shared = BTreeMap<u64, u64>;

const PHASE_WARMUP: u8 = 0;
const PHASE_MEASURE: u8 = 1;
const PHASE_STOP: u8 = 2;

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
    /// Executor balancing-steal interval in polls (0 = off); see
    /// `Executor::with_balance_interval`.
    pub balance_interval: u32,
    /// How a client spends its parallel work after each op.
    pub parallel_mode: ParallelMode,
}

/// How a client spends `parallel_work_cycles` after an op (REVIEW I1).
/// There is no `sleep`: the executor has no timer, and an off-executor
/// timer thread would be a harness-specific wake source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParallelMode {
    /// Spin synchronously in the poll that finished the op (the original
    /// harness; worst case for inline / LIFO placement of a grantee).
    #[default]
    Spin,
    /// `yield_now().await` (back of the local queue), then spin in a later
    /// poll: the op's continuation reaches an await point first.
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

/// The client's parallel work between two ops.
async fn parallel_work(cfg: &Config) {
    if cfg.parallel_mode == ParallelMode::Yield {
        yield_now().await;
    }
    spin_cycles(cfg.parallel_work_cycles);
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

/// Busy-wait for `n` TSC cycles.
#[inline]
pub fn spin_cycles(n: u64) {
    let end = cycles().wrapping_add(n);
    while cycles() < end {
        std::hint::spin_loop();
    }
}

/// Jain fairness index `(sum x)^2 / (n sum x^2)`; `None` if empty or all zero.
pub fn jain<I: IntoIterator<Item = f64>>(xs: I) -> Option<f64> {
    let (mut n, mut s, mut ss) = (0usize, 0.0f64, 0.0f64);
    for x in xs {
        n += 1;
        s += x;
        ss += x * x;
    }
    if n == 0 || ss == 0.0 {
        None
    } else {
        Some(s * s / (n as f64 * ss))
    }
}

/// Estimate TSC frequency (Hz) against the wall clock.
pub fn estimate_tsc_hz() -> f64 {
    let t0 = Instant::now();
    let c0 = cycles();
    std::thread::sleep(Duration::from_millis(100));
    let dc = cycles().wrapping_sub(c0);
    dc as f64 / t0.elapsed().as_secs_f64()
}

struct XorShift64(u64);

impl XorShift64 {
    #[inline]
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
pub struct ClientReport {
    pub id: usize,
    pub class: Class,
    pub ops: u64,
    /// Harness-measured cycles inside the critical section (insert + spin).
    pub service_cycles: u64,
    /// Lock-reported `usage()` (charged cycles), for comparison.
    pub usage_cycles: u64,
    pub combining_cycles: u64,
    /// fc/fcpq: cooperative yields after combining (0 for other locks).
    pub combiner_yields: u64,
    /// `run(cs).await` latency (submission to result).
    pub run_latency: LatencySummary,
}

pub struct ClientResult {
    pub id: usize,
    pub class: Class,
    pub ops: u64,
    pub service_cycles: u64,
    pub usage_cycles: u64,
    pub combining_cycles: u64,
    pub combiner_yields: u64,
    pub run_latency: Histogram,
    /// All ops, including warm-up (sanity mode).
    pub total_ops: u64,
    /// `co-*` locks: handle counters over the measurement window (from the
    /// client's first op in the window to its exit); `None` otherwise.
    pub co: Option<HandleStats>,
}

impl ClientResult {
    fn new(id: usize, class: Class) -> Self {
        ClientResult {
            id,
            class,
            ops: 0,
            service_cycles: 0,
            usage_cycles: 0,
            combining_cycles: 0,
            combiner_yields: 0,
            run_latency: Histogram::new(),
            total_ops: 0,
            co: None,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct ClassReport {
    pub clients: usize,
    pub ops: u64,
    pub service_cycles: u64,
    pub run_latency: LatencySummary,
}

#[derive(Clone, Serialize)]
pub struct WorkerReport {
    pub worker: usize,
    pub cpu: Option<usize>,
    pub combining_cycles: u64,
    pub bystander_latency: LatencySummary,
    /// Bystander samples >= half the measurement window: tasks starved for
    /// (essentially) the whole window and polled only during the drain.
    pub bystander_censored: u64,
    pub client_poll_cycles: u64,
    pub client_polls: u64,
    pub bystander_poll_cycles: u64,
    pub bystander_polls: u64,
    pub steals: u64,
    pub balance_steals: u64,
    pub parks: u64,
    /// Schedules issued from this worker by placement.
    pub placements: PlacementCounts,
    /// Inline-resume chains that ended on this worker (count, p50, max).
    pub chain_lengths: LatencySummary,
    /// Burden (`ces`, `co-*`): critical-section cycles executed here for
    /// requests made on another worker (`stats::record_foreign_cs`).
    pub foreign_cs_cycles: u64,
}

#[derive(Clone, Copy, Default, Serialize)]
pub struct PlacementCounts {
    pub default: u64,
    pub inline: u64,
    pub remote: u64,
    pub home: u64,
}

impl PlacementCounts {
    fn from_array(a: &[u64; 4]) -> Self {
        PlacementCounts {
            default: a[0],
            inline: a[1],
            remote: a[2],
            home: a[3],
        }
    }

    fn add(&mut self, o: &PlacementCounts) {
        self.default += o.default;
        self.inline += o.inline;
        self.remote += o.remote;
        self.home += o.home;
    }
}

impl WorkerReport {
    fn from_stats(s: &WorkerStats, censor_threshold_cycles: u64) -> Self {
        let c = TaskKind::Client.index();
        let b = TaskKind::Bystander.index();
        WorkerReport {
            worker: s.worker,
            cpu: s.cpu,
            combining_cycles: s.combining_cycles,
            bystander_latency: s.bystander_latency.summary(),
            bystander_censored: s.bystander_latency.count_at_least(censor_threshold_cycles),
            client_poll_cycles: s.poll_cycles_by_kind[c],
            client_polls: s.polls_by_kind[c],
            bystander_poll_cycles: s.poll_cycles_by_kind[b],
            bystander_polls: s.polls_by_kind[b],
            steals: s.steals,
            balance_steals: s.balance_steals,
            parks: s.parks,
            placements: PlacementCounts::from_array(&s.placements),
            chain_lengths: s.chain_lengths.summary(),
            foreign_cs_cycles: 0,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct Report {
    pub lock: String,
    pub config: Config,
    pub timestamp: String,
    pub tsc_hz: f64,
    pub measured_secs: f64,
    pub total_ops: u64,
    pub throughput_ops_per_s: f64,
    /// Jain over per-client `service_cycles`.
    pub service_jain: Option<f64>,
    /// Jain over per-worker `combining_cycles`.
    pub burden_jain: Option<f64>,
    pub clients: Vec<ClientReport>,
    pub classes: BTreeMap<String, ClassReport>,
    pub workers: Vec<WorkerReport>,
    /// Bystander schedule->poll latency merged over all workers.
    pub bystander_latency: LatencySummary,
    /// Sum over workers of `combining_cycles`.
    pub total_combining_cycles: u64,
    /// Clients with zero ops in the window (stuck in a never-drained local
    /// queue, e.g. the CES combiner's). Non-zero means the service Jain is
    /// contaminated by an executor placement artifact.
    pub starved_clients: usize,
    /// Bystander samples >= half the window (summed over workers): tasks
    /// that never ran during the window and were polled only at the drain.
    pub starved_bystanders: u64,
    /// Sum over clients of `combiner_yields`.
    pub total_combiner_yields: u64,
    /// Sum over workers of schedules by placement.
    pub placements: PlacementCounts,
    /// Inline-resume chain lengths merged over all workers.
    pub chain_lengths: LatencySummary,
    /// Measurement window in TSC cycles (`measured_secs x tsc_hz`).
    pub window_cycles: f64,
    /// Sum over clients of harness `service_cycles` (critical sections
    /// started in the window).
    pub cs_cycles: u64,
    /// Per-op lock cost `o = (window_cycles - cs_cycles) / total_ops`: the
    /// time the lock (one serial resource) spent not executing a critical
    /// section, per op (REVIEW I3). Per-class op counts are in `classes`.
    pub o_cycles_per_op: Option<f64>,
    /// Jain over per-worker `foreign_cs_cycles` (`ces`, `co-*`; `None` if
    /// nothing was foreign).
    pub burden_foreign_jain: Option<f64>,
    pub total_foreign_cs_cycles: u64,
    /// `co-*` only: handle counters summed over clients (window).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub co_mutex: Option<HandleStats>,
}

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

async fn client_task<L>(lock: Arc<L>, id: usize, cfg: Config, phase: Arc<AtomicU8>) -> ClientResult
where
    L: DelegationLock<Shared>,
    L::Client: ClientExtras,
{
    let class = Class::of(id);
    let cost = match class {
        Class::Light => cfg.light_cs_cycles,
        Class::Heavy => cfg.light_cs_cycles * cfg.heavy_ratio,
    };
    let mut client = lock.client();
    let mut rng = XorShift64(cfg.seed ^ ((id as u64 + 1) * 0x9E37_79B9_7F4A_7C15));
    let mut res = ClientResult::new(id, class);
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
        let dt = client
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
        parallel_work(&cfg).await;
    }
    res.usage_cycles = client.usage();
    res.combining_cycles = client.combining_cycles();
    res.combiner_yields = client.combiner_yields();
    res
}

/// Harness-side access to `co-*` handle counters.
pub trait CoExtras {
    fn co_stats(&self) -> HandleStats;
}

impl<T: Send + 'static, Q: co_mutex::WaitList> CoExtras for co_mutex::CoHandle<T, Q> {
    fn co_stats(&self) -> HandleStats {
        self.stats()
    }
}

/// `client_task` for coroutine-style locks: the same keys, costs, critical
/// section body and measurements, written as the task's own continuation
/// (`lock().await; insert + spin; unlock().await`). `run_latency` spans
/// `lock()` to the return of `unlock().await`, like `run(cs).await`.
async fn co_client_task<M>(
    lock: Arc<M>,
    id: usize,
    cfg: Config,
    phase: Arc<AtomicU8>,
) -> ClientResult
where
    M: CoLock<Shared>,
    M::Handle: CoExtras,
{
    let class = Class::of(id);
    let cost = match class {
        Class::Light => cfg.light_cs_cycles,
        Class::Heavy => cfg.light_cs_cycles * cfg.heavy_ratio,
    };
    let h = lock.handle();
    let mut rng = XorShift64(cfg.seed ^ ((id as u64 + 1) * 0x9E37_79B9_7F4A_7C15));
    let mut res = ClientResult::new(id, class);
    let mut window_start: Option<HandleStats> = None;
    let mut counter: u64 = 0;
    loop {
        let ph = phase.load(Ordering::Relaxed);
        if ph == PHASE_STOP {
            break;
        }
        if ph == PHASE_MEASURE && window_start.is_none() {
            window_start = Some(h.co_stats());
        }
        let key = if cfg.unique_keys {
            ((id as u64) << 40) | counter
        } else {
            rng.next() % cfg.key_space
        };
        counter += 1;
        let t0 = cycles();
        let mut g = h.lock().await;
        let s = cycles();
        g.insert(key, s);
        spin_cycles(cost);
        let dt = cycles().wrapping_sub(s);
        g.unlock().await;
        res.total_ops += 1;
        if ph == PHASE_MEASURE {
            res.ops += 1;
            res.service_cycles += dt;
            res.run_latency.record(cycles().wrapping_sub(t0));
        }
        parallel_work(&cfg).await;
    }
    res.usage_cycles = h.usage();
    let end = h.co_stats();
    res.co = Some(end.since(&window_start.unwrap_or(end)));
    res
}

async fn bystander_task(work: u64, phase: Arc<AtomicU8>) {
    while phase.load(Ordering::Relaxed) != PHASE_STOP {
        spin_cycles(work);
        yield_now().await;
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Everything a run produces before it is folded into a `Report`.
pub struct RawRun {
    pub clients: Vec<ClientResult>,
    pub workers: Vec<WorkerStats>,
    pub measured_secs: f64,
    /// `len()` of the shared map after all clients finished.
    pub final_len: usize,
    /// Per worker: `stats::take_foreign_cs` after the run.
    pub foreign_cs: Vec<u64>,
}

/// Run one configuration on a fresh executor with `lock`.
pub fn run_raw<L>(cfg: &Config, lock: Arc<L>) -> RawRun
where
    L: DelegationLock<Shared>,
    L::Client: ClientExtras,
{
    let read_len = {
        let lock = Arc::clone(&lock);
        async move { lock.client().run(|m: &mut Shared| m.len()).await }
    };
    run_tasks(
        cfg,
        |id, phase| client_task(Arc::clone(&lock), id, cfg.clone(), phase),
        read_len,
    )
}

/// [`run_raw`] for a coroutine-style lock.
pub fn run_raw_co<M>(cfg: &Config, lock: Arc<M>) -> RawRun
where
    M: CoLock<Shared>,
    M::Handle: CoExtras,
{
    let read_len = {
        let lock = Arc::clone(&lock);
        async move {
            let h = lock.handle();
            let g = h.lock().await;
            let n = g.len();
            g.unlock().await;
            n
        }
    };
    run_tasks(
        cfg,
        |id, phase| co_client_task(Arc::clone(&lock), id, cfg.clone(), phase),
        read_len,
    )
}

/// Spawn `client(id, phase)` for every client plus the bystanders, run the
/// warm-up and the window, drain, then `read_len` on a worker.
fn run_tasks<C, F, R>(cfg: &Config, client: C, read_len: R) -> RawRun
where
    C: Fn(usize, Arc<AtomicU8>) -> F,
    F: Future<Output = ClientResult> + Send + 'static,
    R: Future<Output = usize> + Send + 'static,
{
    stats::set_recording(false);
    for w in 0..cfg.workers {
        stats::take_foreign_cs(w);
    }
    let exec = Executor::with_balance_interval(cfg.workers, cfg.balance_interval);
    let phase = Arc::new(AtomicU8::new(if cfg.warmup_ms == 0 {
        PHASE_MEASURE
    } else {
        PHASE_WARMUP
    }));

    // Client id starts on worker id mod workers (deterministic spread; the
    // injector lottery could otherwise park a batch of clients on the future
    // combiner). Later wakes follow the lock's placement.
    let client_tasks: Vec<_> = (0..cfg.clients)
        .map(|id| {
            exec.spawn_on(
                id % cfg.workers,
                TaskKind::Client,
                client(id, Arc::clone(&phase)),
            )
        })
        .collect();
    // Bystander i starts on worker i mod workers so every worker (in
    // particular the future combiner) hosts one; later wakes may migrate it.
    let bystander_tasks: Vec<_> = (0..cfg.bystanders)
        .map(|i| {
            exec.spawn_on(
                i % cfg.workers,
                TaskKind::Bystander,
                bystander_task(cfg.bystander_work_cycles, Arc::clone(&phase)),
            )
        })
        .collect();

    if cfg.warmup_ms > 0 {
        std::thread::sleep(Duration::from_millis(cfg.warmup_ms));
    }
    stats::set_recording(true);
    phase.store(PHASE_MEASURE, Ordering::SeqCst);
    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(cfg.duration_ms));
    phase.store(PHASE_STOP, Ordering::SeqCst);
    let measured_secs = start.elapsed().as_secs_f64();

    // Recording stays on through the drain: a bystander starved for the whole
    // window is polled for the first time only now, and that poll records its
    // full (censored) wait. Client ops are gated by `phase` instead.
    let clients = exec.block_on(async {
        let mut out = Vec::with_capacity(client_tasks.len());
        for t in client_tasks {
            out.push(t.await);
        }
        for t in bystander_tasks {
            t.await;
        }
        out
    });
    stats::set_recording(false);

    // Read the map length through the lock, on a worker (delegation locks may
    // need `worker_id()`).
    let len_task = exec.spawn(TaskKind::Client, read_len);
    let final_len = exec.block_on(len_task);

    let workers = exec.shutdown();
    let foreign_cs = (0..cfg.workers).map(stats::take_foreign_cs).collect();
    RawRun {
        clients,
        workers,
        measured_secs,
        final_len,
        foreign_cs,
    }
}

pub fn make_report(lock_name: &str, cfg: &Config, raw: &RawRun, tsc_hz: f64) -> Report {
    let mut clients: Vec<ClientReport> = raw
        .clients
        .iter()
        .map(|c| ClientReport {
            id: c.id,
            class: c.class,
            ops: c.ops,
            service_cycles: c.service_cycles,
            usage_cycles: c.usage_cycles,
            combining_cycles: c.combining_cycles,
            combiner_yields: c.combiner_yields,
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

    let censor_threshold = (0.5 * raw.measured_secs * tsc_hz) as u64;
    let workers: Vec<WorkerReport> = raw
        .workers
        .iter()
        .map(|w| WorkerReport {
            foreign_cs_cycles: raw.foreign_cs.get(w.worker).copied().unwrap_or(0),
            ..WorkerReport::from_stats(w, censor_threshold)
        })
        .collect();
    let mut bystander = Histogram::new();
    for w in &raw.workers {
        bystander.merge(&w.bystander_latency);
    }
    let total_ops: u64 = clients.iter().map(|c| c.ops).sum();
    let total_combining_cycles: u64 = workers.iter().map(|w| w.combining_cycles).sum();
    let starved_clients = clients.iter().filter(|c| c.ops == 0).count();
    let starved_bystanders: u64 = workers.iter().map(|w| w.bystander_censored).sum();
    let total_combiner_yields: u64 = clients.iter().map(|c| c.combiner_yields).sum();
    let mut placements = PlacementCounts::default();
    let mut chains = Histogram::new();
    for w in &raw.workers {
        placements.add(&PlacementCounts::from_array(&w.placements));
        chains.merge(&w.chain_lengths);
    }
    let window_cycles = raw.measured_secs * tsc_hz;
    let cs_cycles: u64 = clients.iter().map(|c| c.service_cycles).sum();
    let total_foreign_cs_cycles: u64 = workers.iter().map(|w| w.foreign_cs_cycles).sum();
    let burden_foreign_jain = jain(workers.iter().map(|w| w.foreign_cs_cycles as f64));
    let co_mutex = raw.clients.iter().filter_map(|c| c.co).reduce(|mut a, b| {
        a.add(&b);
        a
    });
    Report {
        lock: lock_name.to_string(),
        config: cfg.clone(),
        timestamp: timestamp(),
        tsc_hz,
        measured_secs: raw.measured_secs,
        total_ops,
        throughput_ops_per_s: total_ops as f64 / raw.measured_secs,
        service_jain: jain(clients.iter().map(|c| c.service_cycles as f64)),
        burden_jain: jain(workers.iter().map(|w| w.combining_cycles as f64)),
        clients,
        classes,
        workers,
        bystander_latency: bystander.summary(),
        total_combining_cycles,
        starved_clients,
        starved_bystanders,
        total_combiner_yields,
        placements,
        chain_lengths: chains.summary(),
        window_cycles,
        cs_cycles,
        o_cycles_per_op: (total_ops > 0)
            .then(|| (window_cycles - cs_cycles as f64) / total_ops as f64),
        burden_foreign_jain,
        total_foreign_cs_cycles,
        co_mutex,
    }
}

/// Benchmark `L` with `cfg`, constructing the lock with `make`; `label` is
/// the variant id written to the report (`L::name()` plus option suffixes).
pub fn run_benchmark<L>(
    cfg: &Config,
    tsc_hz: f64,
    label: &str,
    make: impl FnOnce(Shared) -> L,
) -> Report
where
    L: DelegationLock<Shared>,
    L::Client: ClientExtras,
{
    let lock = Arc::new(make(Shared::new()));
    let raw = run_raw(cfg, lock);
    make_report(label, cfg, &raw, tsc_hz)
}

/// [`run_benchmark`] for a coroutine-style lock.
pub fn run_co_benchmark<M>(
    cfg: &Config,
    tsc_hz: f64,
    label: &str,
    make: impl FnOnce(Shared) -> M,
) -> Report
where
    M: CoLock<Shared>,
    M::Handle: CoExtras,
{
    let lock = Arc::new(make(Shared::new()));
    let raw = run_raw_co(cfg, lock);
    make_report(label, cfg, &raw, tsc_hz)
}

/// Mutual-exclusion check: unique keys per client, so the map length must
/// equal the number of completed inserts. Returns `Err` with a description
/// on failure.
pub fn sanity<L>(cfg: &Config, make: impl FnOnce(Shared) -> L) -> Result<SanityOutcome, String>
where
    L: DelegationLock<Shared>,
    L::Client: ClientExtras,
{
    let cfg = Config {
        unique_keys: true,
        ..cfg.clone()
    };
    let lock = Arc::new(make(Shared::new()));
    check_sanity(L::name(), &run_raw(&cfg, lock))
}

/// [`sanity`] for a coroutine-style lock.
pub fn sanity_co<M>(cfg: &Config, make: impl FnOnce(Shared) -> M) -> Result<SanityOutcome, String>
where
    M: CoLock<Shared>,
    M::Handle: CoExtras,
{
    let cfg = Config {
        unique_keys: true,
        ..cfg.clone()
    };
    let lock = Arc::new(make(Shared::new()));
    check_sanity(M::name(), &run_raw_co(&cfg, lock))
}

fn check_sanity(lock: &'static str, raw: &RawRun) -> Result<SanityOutcome, String> {
    let total_ops: u64 = raw.clients.iter().map(|c| c.total_ops).sum();
    let outcome = SanityOutcome {
        lock,
        total_ops,
        final_len: raw.final_len,
    };
    if raw.final_len as u64 == total_ops {
        Ok(outcome)
    } else {
        Err(format!(
            "{lock}: map len {} != total ops {total_ops} (mutual exclusion violated)",
            raw.final_len,
        ))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SanityOutcome {
    pub lock: &'static str,
    pub total_ops: u64,
    pub final_len: usize,
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}
