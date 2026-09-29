//! Flat combining with usage-ordered service (FC-PQ) on the delegation core
//! of [`super::fc`], mirroring `crates/libdlock/src/dlock2/fc_pq/lock.rs`:
//!
//! - the combiner serves the pending request with the lowest cumulative
//!   charged cycles first (binary min-heap keyed by `(usage, arrival)`);
//! - each closure is charged `cycles()` around its execution; the charge
//!   persists in the client's node across requests;
//! - a newcomer (a client whose request has never been served) enters at
//!   [`FcPqOptions::newcomer_init`]; default [`NewcomerInit::Mean`], the
//!   running mean cost of one served request, so it cannot starve everyone
//!   by looking cheap. Nothing else is re-initialised: a client that was
//!   away keeps its cumulative charge;
//! - an entry that has waited more than [`FcPqOptions::starvation_clamp`]
//!   passes (default [`STARVATION_THRESHOLD`]; 0 = never) has
//!   its *key* clamped to the current heap minimum at the start of every
//!   pass until it is served. Deviation from the reference, deliberately:
//!   the reference clamps *after* popping the minimum, which is a no-op, and
//!   had it worked it would have overwritten the client's accounting. Here
//!   the clamp is a bounded-wait promotion for the current queue stay only;
//!   `usage()` stays the cumulative charge (minus `credit_combining`).
//! - at most `pass_limit` closures per pass ([`super::fc::DEFAULT_PASS_LIMIT`]).
//!
//! With [`FcPqOptions::record_waits`], queue waits (passes from admission to
//! service) are counted in [`WaitStats`] during the measurement window; see
//! [`take_wait_stats`].
//!
//! [`FcPqOptions`] adds the H-D mitigations, each independently switchable:
//! `pass_budget_cycles`, `rotate_combiner`, `credit_combining`,
//! `elect_max_usage`. See [`Policy`] for where each one hooks in.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::ptr::NonNull;
use std::sync::Mutex;

use super::fc::{Client, Core, FcOptions, Node, Policy, WakePlacement};
use crate::stats;

/// Default of [`FcPqOptions::starvation_clamp`]: passes an entry may wait
/// before its key is clamped to the heap minimum.
pub const STARVATION_THRESHOLD: u64 = 8;

/// Usage a client's first request is admitted with (it has no charge yet).
/// Only requests of clients with `served() == 0` are affected, i.e. one
/// request per client per lock lifetime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NewcomerInit {
    /// Running mean cost of one served request (`sum of charges / served`);
    /// the client's own (zero) usage before the first service anywhere.
    #[default]
    Mean,
    /// Zero.
    Zero,
    /// Smallest accounting usage among the currently queued entries (the
    /// policy's only view of other clients); 0 if none is queued.
    Min,
    /// Lower median of the currently queued entries' accounting usage; 0 if
    /// none is queued.
    Median,
}

/// Wait histogram buckets: waits of 0..`WAIT_BUCKETS - 1` passes exactly,
/// the last bucket also holds every longer wait.
pub const WAIT_BUCKETS: usize = 64;

/// Queue-wait accounting of one lock, in combining passes: a request
/// admitted in pass `a` (drained at its start, or at the end of pass `a`)
/// and served in pass `s` waited `s - a`. Counted only with
/// [`FcPqOptions::record_waits`] and while [`stats::recording`] is on
/// (measurement window plus drain).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitStats {
    /// Passes begun with a non-empty queue.
    pub passes: u64,
    /// Requests served.
    pub served: u64,
    /// Served requests whose key the starvation clamp had lowered.
    pub promoted: u64,
    /// Longest wait, passes.
    pub max_wait: u64,
    pub wait_hist: [u64; WAIT_BUCKETS],
}

impl WaitStats {
    const fn new() -> Self {
        Self {
            passes: 0,
            served: 0,
            promoted: 0,
            max_wait: 0,
            wait_hist: [0; WAIT_BUCKETS],
        }
    }

    fn record(&mut self, wait: u64, promoted: bool) {
        self.served += 1;
        self.promoted += promoted as u64;
        self.max_wait = self.max_wait.max(wait);
        self.wait_hist[(wait as usize).min(WAIT_BUCKETS - 1)] += 1;
    }
}

/// Stats of the last dropped lock that served anything while recording.
static LAST_WAIT_STATS: Mutex<Option<WaitStats>> = Mutex::new(None);

/// Take the [`WaitStats`] of the most recently dropped [`FcPq`] that served
/// a request while recording (the harness drops the lock when `run_raw`
/// returns), clearing the slot.
pub fn take_wait_stats() -> Option<WaitStats> {
    LAST_WAIT_STATS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// Combiner-fairness mitigations (RESEARCH.md H-D) plus the executor-facing
/// knobs of [`FcOptions`]. `Default` = every mitigation off,
/// `yield_after_combine` on, default placement, pass limit
/// [`super::fc::DEFAULT_PASS_LIMIT`]: plain FC-PQ that cooperates with the
/// executor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FcPqOptions {
    /// Stop a pass once this many cycles have elapsed, in addition to
    /// `pass_limit`.
    pub pass_budget_cycles: Option<u64>,
    /// At every pass end with work left, hand the combiner role to the next
    /// waiter instead of running another pass, even if the combiner's own
    /// request is still queued.
    pub rotate_combiner: bool,
    /// Subtract the combiner's combining cycles (pass wall time minus its own
    /// critical section) from its usage, floored at zero.
    pub credit_combining: bool,
    /// Hand the combiner role to the waiting client with the highest usage
    /// instead of the heap minimum.
    pub elect_max_usage: bool,
    /// See [`FcOptions::yield_after_combine`].
    pub yield_after_combine: bool,
    /// See [`FcOptions::wake_placement`].
    pub wake_placement: WakePlacement,
    /// See [`FcOptions::pass_limit`].
    pub pass_limit: usize,
    /// Passes a queued request may wait before its key is clamped to the
    /// heap minimum; 0 disables the clamp. Default [`STARVATION_THRESHOLD`].
    pub starvation_clamp: u64,
    /// Usage a client's first request enters with. Default
    /// [`NewcomerInit::Mean`].
    pub newcomer_init: NewcomerInit,
    /// Count queue waits in [`WaitStats`] (two counter updates per served
    /// request). Off by default; with the counters boxed off the hot lines
    /// its cost was inside the run-to-run spread (FINDINGS 2026-09-29).
    pub record_waits: bool,
}

impl Default for FcPqOptions {
    fn default() -> Self {
        let core = FcOptions::default();
        Self {
            pass_budget_cycles: None,
            rotate_combiner: false,
            credit_combining: false,
            elect_max_usage: false,
            yield_after_combine: core.yield_after_combine,
            wake_placement: core.wake_placement,
            pass_limit: core.pass_limit,
            starvation_clamp: STARVATION_THRESHOLD,
            newcomer_init: NewcomerInit::Mean,
            record_waits: false,
        }
    }
}

impl FcPqOptions {
    /// The subset the shared [`Core`] consumes.
    pub fn core(&self) -> FcOptions {
        FcOptions {
            yield_after_combine: self.yield_after_combine,
            wake_placement: self.wake_placement,
            pass_limit: self.pass_limit,
        }
    }
}

struct Entry<T> {
    /// Scheduling key: `base`, possibly clamped by the anti-starvation rule.
    key: u64,
    /// Accounting base: the node's usage at admission, or the running mean
    /// for a newcomer. The charge after service is `base + cs`.
    base: u64,
    /// Arrival sequence; FIFO among equal keys.
    seq: u64,
    /// Pass in which the entry was admitted.
    pass_entered: u64,
    node: NonNull<Node<T>>,
}

impl<T> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.seq == other.seq
    }
}
impl<T> Eq for Entry<T> {}
impl<T> PartialOrd for Entry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Entry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.key, self.seq).cmp(&(other.key, other.seq))
    }
}

/// Usage-ordered service policy.
pub struct UsagePq<T> {
    heap: BinaryHeap<Reverse<Entry<T>>>,
    opts: FcPqOptions,
    /// Running totals over served requests, for newcomer initialisation.
    total_usage: u64,
    served: u64,
    seq: u64,
    /// Request handed out by the last `next`, with its key.
    current: Option<(NonNull<Node<T>>, u64)>,
    /// Newcomer-median workspace and wait accounting, boxed so the fields
    /// the combiner touches on every request stay on as few cache lines as
    /// before they were added (inline, they cost ~0.5 % throughput even
    /// with `record_waits` off).
    cold: Box<Cold>,
}

struct Cold {
    /// Pass most recently begun (maintained only with `record_waits`).
    pass: u64,
    /// Sized with the heap.
    scratch: Vec<u64>,
    wait: WaitStats,
}

// SAFETY: the queued pointers are dereferenced only by the combiner.
unsafe impl<T> Send for UsagePq<T> {}

impl<T> UsagePq<T> {
    pub fn new(opts: FcPqOptions) -> Self {
        Self {
            // Bounded by the number of concurrently pending requests (one per
            // client); sized so the steady state never reallocates.
            heap: BinaryHeap::with_capacity(1024),
            opts,
            total_usage: 0,
            served: 0,
            seq: 0,
            current: None,
            cold: Box::new(Cold {
                pass: 0,
                scratch: Vec::with_capacity(1024),
                wait: WaitStats::new(),
            }),
        }
    }

    pub fn options(&self) -> FcPqOptions {
        self.opts
    }

    /// Rewrite keys in place and restore the heap. `BinaryHeap::from(Vec)`
    /// reuses the allocation, so this is O(n) without allocating.
    fn rekey(&mut self, mut f: impl FnMut(&mut Entry<T>)) {
        let mut v = std::mem::take(&mut self.heap).into_vec();
        for e in &mut v {
            f(&mut e.0);
        }
        self.heap = BinaryHeap::from(v);
    }

    /// Admission usage of a newcomer whose own usage is `own`
    /// ([`FcPqOptions::newcomer_init`]).
    fn newcomer_base(&mut self, own: u64) -> u64 {
        match self.opts.newcomer_init {
            NewcomerInit::Mean if self.served > 0 => self.total_usage / self.served,
            NewcomerInit::Mean => own,
            NewcomerInit::Zero => 0,
            NewcomerInit::Min => self.heap.iter().map(|e| e.0.base).min().unwrap_or(0),
            NewcomerInit::Median => {
                let scratch = &mut self.cold.scratch;
                scratch.clear();
                scratch.extend(self.heap.iter().map(|e| e.0.base));
                if scratch.is_empty() {
                    return 0;
                }
                let mid = (scratch.len() - 1) / 2;
                *scratch.select_nth_unstable(mid).1
            }
        }
    }
}

impl<T> Drop for UsagePq<T> {
    fn drop(&mut self) {
        if self.cold.wait.served > 0 {
            *LAST_WAIT_STATS.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.cold.wait);
        }
    }
}

impl<T> Default for UsagePq<T> {
    fn default() -> Self {
        Self::new(FcPqOptions::default())
    }
}

impl<T: Send + 'static> Policy<T> for UsagePq<T> {
    const NAME: &'static str = "fcpq";

    fn admit(&mut self, node: NonNull<Node<T>>, pass: u64) {
        // SAFETY: admitted nodes are PENDING and live as long as the lock.
        let n = unsafe { node.as_ref() };
        let mut base = n.usage();
        if n.served() == 0 {
            base = self.newcomer_base(base);
        }
        let seq = self.seq;
        self.seq += 1;
        self.heap.push(Reverse(Entry {
            key: base,
            base,
            seq,
            pass_entered: pass,
            node,
        }));
    }

    fn begin_pass(&mut self, pass: u64) {
        if self.opts.record_waits {
            self.cold.pass = pass;
            if !self.heap.is_empty() && stats::recording() {
                self.cold.wait.passes += 1;
            }
        }
        let clamp = self.opts.starvation_clamp;
        if clamp == 0 {
            return;
        }
        let Some(min) = self.heap.peek().map(|e| e.0.key) else {
            return;
        };
        let starving = |e: &Entry<T>| pass - e.pass_entered > clamp;
        if self.heap.iter().any(|e| starving(&e.0)) {
            self.rekey(|e| {
                if starving(e) {
                    e.key = e.key.min(min);
                }
            });
        }
    }

    fn next(&mut self) -> Option<NonNull<Node<T>>> {
        let Reverse(e) = self.heap.pop()?;
        if self.opts.record_waits && stats::recording() {
            let wait = self.cold.pass.saturating_sub(e.pass_entered);
            self.cold.wait.record(wait, e.key < e.base);
        }
        self.current = Some((e.node, e.base));
        Some(e.node)
    }

    fn charge(&mut self, cs: u64) {
        let (node, base) = self.current.expect("charge without next");
        // SAFETY: `node` came from `next` and is still PENDING.
        unsafe { node.as_ref() }.charge(base + cs);
        self.total_usage += cs;
        self.served += 1;
    }

    fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    fn candidate(&self) -> Option<NonNull<Node<T>>> {
        if self.opts.elect_max_usage {
            // Highest charged usage; oldest arrival among equals.
            self.heap
                .iter()
                .max_by(|a, b| (a.0.base, Reverse(a.0.seq)).cmp(&(b.0.base, Reverse(b.0.seq))))
                .map(|e| e.0.node)
        } else {
            self.heap.peek().map(|e| e.0.node)
        }
    }

    fn pass_budget(&self) -> Option<u64> {
        self.opts.pass_budget_cycles
    }

    fn rotate(&self) -> bool {
        self.opts.rotate_combiner
    }

    fn credit(&mut self, node: NonNull<Node<T>>, cycles: u64) {
        if !self.opts.credit_combining || cycles == 0 {
            return;
        }
        // If the combiner's own request is still queued, its entry carries
        // the accounting; otherwise the node holds the live value.
        if self.heap.iter().any(|e| e.0.node == node) {
            self.rekey(|e| {
                if e.node == node {
                    e.base = e.base.saturating_sub(cycles);
                    e.key = e.key.saturating_sub(cycles);
                }
            });
        } else {
            // SAFETY: the combiner's node lives as long as the lock.
            let n = unsafe { node.as_ref() };
            n.set_usage(n.usage().saturating_sub(cycles));
        }
    }
}

/// Flat combining, usage-ordered service.
pub type FcPq<T> = Core<T, UsagePq<T>>;

/// Client handle of [`FcPq`].
pub type FcPqClient<T> = Client<T, UsagePq<T>>;

impl<T: Send + 'static> FcPq<T> {
    pub fn with_options(data: T, opts: FcPqOptions) -> Self {
        Core::with_policy(data, UsagePq::new(opts), opts.core())
    }
}

#[cfg(test)]
mod tests {
    use super::super::fc::testing::{run_all, run_threads, spin_cycles, BoxFut};
    use super::*;
    use crate::lock::{DelegationLock, LockClient};
    use std::sync::Arc;
    use std::time::Duration;

    const STALL: Duration = Duration::from_secs(5);
    const LIGHT: u64 = 4_000;
    const HEAVY: u64 = 8 * LIGHT;

    fn task(mut client: FcPqClient<u64>, ops: usize, cs: u64) -> BoxFut {
        Box::pin(async move {
            for _ in 0..ops {
                client
                    .run(move |c| {
                        *c += 1;
                        spin_cycles(cs);
                    })
                    .await;
            }
        })
    }

    fn count(lock: &FcPq<u64>) -> u64 {
        *lock.data()
    }

    /// 16 clients (8 light, 8 heavy) × `ops`; returns (count, light usages,
    /// heavy usages).
    fn heterogeneous(
        lock: &Arc<FcPq<u64>>,
        threads: usize,
        ops: usize,
    ) -> (u64, Vec<u64>, Vec<u64>) {
        let n = 16;
        let clients: Vec<_> = (0..n).map(|_| lock.client()).collect();
        let nodes: Vec<NonNull<Node<u64>>> = clients.iter().map(|c| c.node_ptr()).collect();
        let mut slices: Vec<Vec<BoxFut>> = (0..threads).map(|_| Vec::new()).collect();
        for (i, c) in clients.into_iter().enumerate() {
            let cs = if i % 2 == 0 { LIGHT } else { HEAVY };
            slices[i % threads].push(task(c, ops, cs));
        }
        if threads == 1 {
            run_all(slices.pop().unwrap(), STALL);
        } else {
            run_threads(slices, STALL);
        }
        let usage = |i: usize| unsafe { nodes[i].as_ref() }.usage();
        let light = (0..n).step_by(2).map(usage).collect();
        let heavy = (1..n).step_by(2).map(usage).collect();
        (count(lock), light, heavy)
    }

    fn mean(v: &[u64]) -> f64 {
        v.iter().sum::<u64>() as f64 / v.len() as f64
    }

    #[test]
    fn fcpq_16_clients_1000_ops_single_thread_counts_and_charges_8x() {
        let lock = Arc::new(FcPq::<u64>::new(0));
        let (count, light, heavy) = heterogeneous(&lock, 1, 1000);
        assert_eq!(count, 16_000);
        let ratio = mean(&heavy) / mean(&light);
        assert!(
            (6.5..=9.5).contains(&ratio),
            "heavy/light usage ratio {ratio:.2}, light {light:?}, heavy {heavy:?}"
        );
        // Every charge covers the spin inside the closure.
        for u in &light {
            assert!(*u >= 1000 * LIGHT);
        }
        for u in &heavy {
            assert!(*u >= 1000 * HEAVY);
        }
    }

    #[test]
    fn fcpq_multi_thread_counts_and_charges_8x() {
        let lock = Arc::new(FcPq::<u64>::new(0));
        let (count, light, heavy) = heterogeneous(&lock, 4, 2000);
        assert_eq!(count, 32_000);
        let ratio = mean(&heavy) / mean(&light);
        assert!((6.5..=9.5).contains(&ratio), "ratio {ratio:.2}");
    }

    fn option_matrix() -> Vec<(&'static str, FcPqOptions)> {
        let base = FcPqOptions::default();
        vec![
            (
                "budget",
                FcPqOptions {
                    pass_budget_cycles: Some(LIGHT * 3),
                    ..base
                },
            ),
            (
                "rotate",
                FcPqOptions {
                    rotate_combiner: true,
                    ..base
                },
            ),
            (
                "credit",
                FcPqOptions {
                    credit_combining: true,
                    ..base
                },
            ),
            (
                "elect_max",
                FcPqOptions {
                    elect_max_usage: true,
                    ..base
                },
            ),
            (
                "all",
                FcPqOptions {
                    pass_budget_cycles: Some(LIGHT * 3),
                    rotate_combiner: true,
                    credit_combining: true,
                    elect_max_usage: true,
                    yield_after_combine: true,
                    wake_placement: WakePlacement::Default,
                    pass_limit: 16,
                    starvation_clamp: 32,
                    newcomer_init: NewcomerInit::Median,
                    record_waits: true,
                },
            ),
            (
                "clamp_off",
                FcPqOptions {
                    starvation_clamp: 0,
                    ..base
                },
            ),
            (
                "newcomer_min",
                FcPqOptions {
                    newcomer_init: NewcomerInit::Min,
                    ..base
                },
            ),
            (
                "pass_limit_8",
                FcPqOptions {
                    pass_limit: 8,
                    ..base
                },
            ),
            (
                "noyield",
                FcPqOptions {
                    yield_after_combine: false,
                    ..base
                },
            ),
        ]
    }

    #[test]
    fn fcpq_options_single_thread_stay_correct_and_live() {
        for (name, opts) in option_matrix() {
            let lock = Arc::new(FcPq::with_options(0u64, opts));
            let (count, light, heavy) = heterogeneous(&lock, 1, 500);
            assert_eq!(count, 8_000, "{name}");
            if !opts.credit_combining {
                let ratio = mean(&heavy) / mean(&light);
                assert!((6.0..=10.0).contains(&ratio), "{name}: ratio {ratio:.2}");
            }
        }
    }

    #[test]
    fn fcpq_options_multi_thread_stay_correct_and_live() {
        for (name, opts) in option_matrix() {
            let lock = Arc::new(FcPq::with_options(0u64, opts));
            let (count, _, _) = heterogeneous(&lock, 4, 1000);
            assert_eq!(count, 16_000, "{name}");
        }
    }

    /// `pass_limit = 1`: with four requests queued while the flag is held,
    /// the combiner serves exactly one per pass in usage order, so the heavy
    /// (high-usage) client is deferred to the fourth pass even though it is
    /// the combiner itself.
    #[test]
    fn pass_limit_one_serves_in_usage_order_across_passes() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        let lock = Arc::new(FcPq::with_options(
            Vec::<usize>::new(),
            FcPqOptions {
                pass_limit: 1,
                yield_after_combine: false,
                ..FcPqOptions::default()
            },
        ));
        let mut clients: Vec<_> = (0..4).map(|_| lock.client()).collect();
        // Client 3 is the heavy one: prior usage far above the others.
        unsafe { clients[3].node_ptr().as_ref() }.charge(1_000_000);

        // Hold the flag so every first poll publishes and loses the election.
        assert!(lock.hold_combiner());
        let mut cx = Context::from_waker(Waker::noop());
        let mut runs: Vec<_> = clients
            .iter_mut()
            .enumerate()
            .map(|(i, c)| Box::pin(c.run(move |log: &mut Vec<usize>| log.push(i))))
            .collect();
        for r in &mut runs {
            assert!(r.as_mut().poll(&mut cx).is_pending());
        }
        lock.release_combiner();

        // The heavy client re-polls, wins, and must combine until its own
        // request is served: four passes of one closure each.
        let mut heavy = runs.pop().unwrap();
        assert!(heavy.as_mut().poll(&mut cx).is_ready());
        assert_eq!(lock.passes(), 4);
        for r in &mut runs {
            assert!(matches!(r.as_mut().poll(&mut cx), Poll::Ready(())));
        }
        drop(heavy);
        drop(runs);
        drop(clients);
        let log = Arc::try_unwrap(lock)
            .ok()
            .expect("clients gone")
            .into_inner();
        assert_eq!(log, vec![0, 1, 2, 3]);
    }

    #[test]
    fn starvation_clamp_promotes_old_entries() {
        // Direct policy test: a high-usage entry that sat through more than
        // STARVATION_THRESHOLD passes is served before a fresh cheap one.
        let lock = Arc::new(FcPq::<u64>::new(0));
        let heavy = lock.client();
        let light = lock.client();
        let (heavy, light) = (heavy.node_ptr(), light.node_ptr());
        let mut pq = UsagePq::<u64>::new(FcPqOptions::default());
        // Give the heavy node prior usage, the light node none.
        unsafe { heavy.as_ref() }.charge(1_000_000);
        pq.served = 1;
        pq.total_usage = 10;
        pq.admit(heavy, 1);
        pq.begin_pass(1);
        assert_eq!(pq.candidate(), Some(heavy));
        // Newcomer at pass 20 with zero usage → running mean (10).
        pq.admit(light, 20);
        assert_eq!(pq.candidate(), Some(light));
        // Heavy has waited 19 passes: clamped to the min, and older seq wins.
        pq.begin_pass(20);
        assert_eq!(pq.next(), Some(heavy));
        assert_eq!(pq.next(), Some(light));
        assert!(pq.next().is_none());
    }

    /// Heavy entry (usage 1e6) admitted in pass 1, cheap newcomer in pass
    /// 20; returns the service order at pass `at` under `clamp`.
    fn clamp_order(clamp: u64, at: u64) -> (bool, bool) {
        let lock = Arc::new(FcPq::<u64>::new(0));
        let (heavy, light) = (lock.client().node_ptr(), lock.client().node_ptr());
        let mut pq = UsagePq::<u64>::new(FcPqOptions {
            starvation_clamp: clamp,
            ..FcPqOptions::default()
        });
        unsafe { heavy.as_ref() }.charge(1_000_000);
        pq.served = 1;
        pq.total_usage = 10;
        pq.admit(heavy, 1);
        pq.admit(light, 20);
        pq.begin_pass(at);
        let first = pq.next();
        let second = pq.next();
        assert!(pq.next().is_none());
        (first == Some(heavy), second == Some(light))
    }

    #[test]
    fn starvation_clamp_threshold_is_configurable_and_zero_disables_it() {
        // Waited 19 passes: promoted under the default 8, not under 32 or 0.
        assert_eq!(clamp_order(STARVATION_THRESHOLD, 20), (true, true));
        assert_eq!(clamp_order(32, 20), (false, false));
        assert_eq!(clamp_order(0, 20), (false, false));
        // 32 fires once the wait exceeds 32 passes; 0 never does.
        assert_eq!(clamp_order(32, 33), (false, false));
        assert_eq!(clamp_order(32, 34), (true, true));
        assert_eq!(clamp_order(0, 1_000_000), (false, false));
    }

    #[test]
    fn newcomer_init_modes() {
        let check = |init: NewcomerInit, expect: u64| {
            let lock = Arc::new(FcPq::<u64>::new(0));
            let mut pq = UsagePq::<u64>::new(FcPqOptions {
                newcomer_init: init,
                ..FcPqOptions::default()
            });
            pq.served = 4;
            pq.total_usage = 40;
            // Queued established clients with usages 900, 100, 200, 5000.
            for u in [900, 100, 200, 5000] {
                let n = lock.client().node_ptr();
                unsafe { n.as_ref() }.charge(u);
                pq.admit(n, 1);
            }
            let newcomer = lock.client().node_ptr();
            pq.admit(newcomer, 1);
            let e = pq.heap.iter().find(|e| e.0.node == newcomer).unwrap();
            assert_eq!((e.0.base, e.0.key), (expect, expect), "{init:?}");
        };
        check(NewcomerInit::Mean, 10);
        check(NewcomerInit::Zero, 0);
        check(NewcomerInit::Min, 100);
        // Lower median of {100, 200, 900, 5000}.
        check(NewcomerInit::Median, 200);

        // Empty queue: min / median fall back to 0; established clients keep
        // their usage under every mode.
        for init in [NewcomerInit::Min, NewcomerInit::Median] {
            let lock = Arc::new(FcPq::<u64>::new(0));
            let mut pq = UsagePq::<u64>::new(FcPqOptions {
                newcomer_init: init,
                ..FcPqOptions::default()
            });
            let (a, b) = (lock.client().node_ptr(), lock.client().node_ptr());
            pq.admit(a, 1);
            unsafe { b.as_ref() }.charge(777);
            pq.admit(b, 1);
            let base = |n| pq.heap.iter().find(|e| e.0.node == n).unwrap().0.base;
            assert_eq!((base(a), base(b)), (0, 777), "{init:?}");
        }
    }
}
