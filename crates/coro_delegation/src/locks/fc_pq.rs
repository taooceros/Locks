//! Flat combining with usage-ordered service (FC-PQ) on the delegation core
//! of [`super::fc`], mirroring `crates/libdlock/src/dlock2/fc_pq/lock.rs`:
//!
//! - the combiner serves the pending request with the lowest cumulative
//!   charged cycles first (binary min-heap keyed by `(usage, arrival)`);
//! - each closure is charged `cycles()` around its execution; the charge
//!   persists in the client's node across requests;
//! - a newcomer with zero usage enters at the running mean of all served
//!   requests so it cannot starve everyone by looking cheap;
//! - an entry that has waited more than [`STARVATION_THRESHOLD`] passes has
//!   its *key* clamped to the current heap minimum at the start of every
//!   pass until it is served. Deviation from the reference, deliberately:
//!   the reference clamps *after* popping the minimum, which is a no-op, and
//!   had it worked it would have overwritten the client's accounting. Here
//!   the clamp is a bounded-wait promotion for the current queue stay only;
//!   `usage()` stays the cumulative charge (minus `credit_combining`).
//! - at most `pass_limit` closures per pass ([`super::fc::DEFAULT_PASS_LIMIT`]).
//!
//! [`FcPqOptions`] adds the H-D mitigations, each independently switchable:
//! `pass_budget_cycles`, `rotate_combiner`, `credit_combining`,
//! `elect_max_usage`. See [`Policy`] for where each one hooks in.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::ptr::NonNull;

use super::fc::{Client, Core, FcOptions, Node, Policy, WakePlacement};

/// Passes an entry may wait before its key is clamped to the heap minimum.
pub const STARVATION_THRESHOLD: u64 = 8;

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
        if n.served() == 0 && self.served > 0 {
            base = self.total_usage / self.served;
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
        let Some(min) = self.heap.peek().map(|e| e.0.key) else {
            return;
        };
        let starving = |e: &Entry<T>| pass - e.pass_entered > STARVATION_THRESHOLD;
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
}
