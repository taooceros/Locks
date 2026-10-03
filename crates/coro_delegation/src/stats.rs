//! Per-worker statistics: poll cycles and counts by task kind, bystander
//! schedule->poll latency (log-linear histogram) and combining cycles
//! reported by locks via [`record_combining`].
//!
//! Recording is gated by a global flag ([`set_recording`]) so that the
//! harness can exclude warm-up and drain phases; every hot-path update
//! checks it with one relaxed load.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam_utils::CachePadded;
use serde::Serialize;

/// Upper bound on executor workers; `record_combining` ignores larger ids.
pub const MAX_WORKERS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TaskKind {
    Client = 0,
    Bystander = 1,
}

impl TaskKind {
    pub const COUNT: usize = 2;

    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

// ---------------------------------------------------------------------------
// Log-linear histogram
// ---------------------------------------------------------------------------

const SUB_BITS: u32 = 4;
const SUB: usize = 1 << SUB_BITS; // 16 sub-buckets per power of two (~6 % resolution)
const BUCKETS: usize = SUB + (64 - SUB_BITS as usize) * SUB;

/// Fixed-size log-linear histogram of `u64` samples (TSC cycles). Values
/// below 16 are exact; above that each power of two is split into 16
/// linear sub-buckets. Quantiles return the lower bound of the bucket.
#[derive(Clone)]
pub struct Histogram {
    count: u64,
    sum: u64,
    max: u64,
    buckets: Box<[u64; BUCKETS]>,
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    pub fn new() -> Self {
        Histogram {
            count: 0,
            sum: 0,
            max: 0,
            buckets: Box::new([0; BUCKETS]),
        }
    }

    #[inline]
    fn index(v: u64) -> usize {
        if v < SUB as u64 {
            return v as usize;
        }
        let msb = 63 - v.leading_zeros(); // >= SUB_BITS
        let shift = msb - SUB_BITS;
        let sub = ((v >> shift) & (SUB as u64 - 1)) as usize;
        SUB + shift as usize * SUB + sub
    }

    fn lower_bound(idx: usize) -> u64 {
        if idx < SUB {
            return idx as u64;
        }
        let k = (idx - SUB) / SUB;
        let sub = (idx - SUB) % SUB;
        ((SUB + sub) as u64) << k
    }

    #[inline]
    pub fn record(&mut self, v: u64) {
        self.count += 1;
        self.sum += v;
        if v > self.max {
            self.max = v;
        }
        self.buckets[Self::index(v)] += 1;
    }

    pub fn merge(&mut self, other: &Histogram) {
        self.count += other.count;
        self.sum += other.sum;
        self.max = self.max.max(other.max);
        for (a, b) in self.buckets.iter_mut().zip(other.buckets.iter()) {
            *a += *b;
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// Samples whose bucket lower bound is >= `threshold` (bucket-granular,
    /// so samples up to ~6 % below `threshold` may be included).
    pub fn count_at_least(&self, threshold: u64) -> u64 {
        let first = Self::index(threshold);
        self.buckets[first..].iter().sum()
    }

    /// Lower bound of the bucket holding the `q`-quantile (0 < q <= 1); 0 if empty.
    pub fn quantile(&self, q: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let target = ((q * self.count as f64).ceil() as u64).clamp(1, self.count);
        let mut cum = 0u64;
        for (idx, &c) in self.buckets.iter().enumerate() {
            cum += c;
            if cum >= target {
                return Self::lower_bound(idx);
            }
        }
        self.max
    }

    pub fn summary(&self) -> LatencySummary {
        LatencySummary {
            count: self.count,
            mean: if self.count == 0 {
                0.0
            } else {
                self.sum as f64 / self.count as f64
            },
            p50: self.quantile(0.50),
            p99: self.quantile(0.99),
            max: self.max,
        }
    }
}

impl Serialize for Histogram {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.summary().serialize(s)
    }
}

/// Summary of a [`Histogram`] in TSC cycles.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct LatencySummary {
    pub count: u64,
    pub mean: f64,
    pub p50: u64,
    pub p99: u64,
    pub max: u64,
}

// ---------------------------------------------------------------------------
// Worker stats
// ---------------------------------------------------------------------------

/// Statistics owned by one worker thread, returned by `Executor::shutdown`.
#[derive(Clone, Serialize)]
pub struct WorkerStats {
    pub worker: usize,
    /// Logical CPU the worker is pinned to (`None` if pinning failed).
    pub cpu: Option<usize>,
    /// Indexed by `TaskKind::index()`.
    pub poll_cycles_by_kind: [u64; TaskKind::COUNT],
    pub polls_by_kind: [u64; TaskKind::COUNT],
    /// Schedule->poll latency of bystander tasks that were scheduled onto
    /// this worker's queues (filled in by `Executor::shutdown`).
    pub bystander_latency: Histogram,
    /// Cycles locks attributed to this worker via [`record_combining`].
    pub combining_cycles: u64,
    /// Successful steals while idle (from the injector or another worker).
    pub steals: u64,
    /// Successful periodic balancing steals while busy.
    pub balance_steals: u64,
    /// Times the worker parked.
    pub parks: u64,
    /// Schedules issued from this worker by placement, indexed like
    /// `executor::Placement` (default/local, inline, remote, home).
    pub placements: [u64; 4],
    /// Lengths of inline-resume chains that ended on this worker (a chain =
    /// consecutive inline-resumed polls; recorded when a non-inline poll or
    /// shutdown follows).
    pub chain_lengths: Histogram,
}

impl WorkerStats {
    pub fn new(worker: usize, cpu: Option<usize>) -> Self {
        WorkerStats {
            worker,
            cpu,
            poll_cycles_by_kind: [0; TaskKind::COUNT],
            polls_by_kind: [0; TaskKind::COUNT],
            bystander_latency: Histogram::new(),
            combining_cycles: 0,
            steals: 0,
            balance_steals: 0,
            parks: 0,
            placements: [0; 4],
            chain_lengths: Histogram::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Global recording gate and combining counters
// ---------------------------------------------------------------------------

static RECORDING: AtomicBool = AtomicBool::new(false);

static COMBINING: [CachePadded<AtomicU64>; MAX_WORKERS] =
    [const { CachePadded::new(AtomicU64::new(0)) }; MAX_WORKERS];

/// Enable or disable statistics recording (worker poll stats, bystander
/// latency, combining cycles). The harness turns it on for the measurement
/// window only.
pub fn set_recording(on: bool) {
    RECORDING.store(on, Ordering::SeqCst);
}

#[inline]
pub fn recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// Attribute `cycles` of combining work (running other tasks' critical
/// sections plus queue administration) to `worker`. Ignored when recording
/// is off or `worker >= MAX_WORKERS`. Relaxed: the counters are read only
/// after the worker threads have been joined.
#[inline]
pub fn record_combining(worker: usize, cycles: u64) {
    if worker < MAX_WORKERS && recording() {
        COMBINING[worker].fetch_add(cycles, Ordering::Relaxed);
    }
}

/// Read and reset the combining counter of `worker` (executor use).
pub fn take_combining(worker: usize) -> u64 {
    if worker < MAX_WORKERS {
        COMBINING[worker].swap(0, Ordering::Relaxed)
    } else {
        0
    }
}

static FOREIGN_CS: [CachePadded<AtomicU64>; MAX_WORKERS] =
    [const { CachePadded::new(AtomicU64::new(0)) }; MAX_WORKERS];

/// Burden under the uniform definition of REVIEW-2026-09-30 I8, used by
/// `ces` and `co_mutex`: critical-section cycles that `worker` executed for
/// a request whose *home* is another worker. A request's home is the worker
/// that was polling the task when it asked for the lock (the executor's
/// `Placement::Home` target at that moment); an uncontended acquisition is
/// therefore never foreign, and a waiter the lock resumes on the releaser's
/// worker is foreign iff it queued elsewhere. Cycles are the lock's own
/// charge window (`ces`: the closure; `co_mutex`: ownership observed by
/// `lock()` to `unlock()`), attributed to the worker where the critical
/// section started. Queue administration is not included (it is in `o`).
/// Same gating and ordering as [`record_combining`].
#[inline]
pub fn record_foreign_cs(worker: usize, cycles: u64) {
    if worker < MAX_WORKERS && recording() {
        FOREIGN_CS[worker].fetch_add(cycles, Ordering::Relaxed);
    }
}

/// Read and reset the foreign critical-section counter of `worker`
/// (harness use: reset before a run, read after the executor shut down).
pub fn take_foreign_cs(worker: usize) -> u64 {
    if worker < MAX_WORKERS {
        FOREIGN_CS[worker].swap(0, Ordering::Relaxed)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets_round_trip() {
        for v in [0u64, 1, 15, 16, 17, 31, 32, 33, 1000, 1 << 20, u64::MAX] {
            let idx = Histogram::index(v);
            assert!(idx < BUCKETS, "{v}");
            let lb = Histogram::lower_bound(idx);
            assert!(lb <= v, "{v}: lb {lb}");
            // next bucket's lower bound is above v
            if idx + 1 < BUCKETS {
                assert!(Histogram::lower_bound(idx + 1) > v, "{v}");
            }
        }
    }

    #[test]
    fn histogram_quantiles() {
        let mut h = Histogram::new();
        for v in 1..=1000u64 {
            h.record(v);
        }
        let p50 = h.quantile(0.5);
        assert!((480..=500).contains(&p50), "{p50}");
        let p99 = h.quantile(0.99);
        assert!((960..=990).contains(&p99), "{p99}");
    }
}
