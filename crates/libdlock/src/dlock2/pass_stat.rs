//! Combiner pass statistics for the flat-combining locks (FC, FC-PQ).
//!
//! Compiled only with the `combiner_pass_stat` feature (a separate stats build)
//! and in unit tests, so the timed builds carry no counters. The recorder is
//! combiner-only state: each lock keeps it in a `SyncUnsafeCell` that is touched
//! solely under the combiner lock; `pass_stats()` takes that lock for its snapshot.
//!
//! A *pass* is one `combine()` call (a fast-path request counts as a one-body
//! pass, as it is charged like one). `bodies` counts delegate executions, not
//! queue pops. Empty passes (no body run) are counted in `passes` and
//! `empty_passes`; ratios of totals, not per-pass averages, are the headline.

/// Histogram buckets: bodies per pass 0..=64, and index 65 for 65 or more.
pub const HIST_BUCKETS: usize = 66;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassStats {
    /// `combine()` calls plus fast-path requests.
    pub passes: u64,
    /// Passes that ran no body.
    pub empty_passes: u64,
    /// Delegate executions over all passes.
    pub bodies: u64,
    /// Passes whose combiner differs from the previous pass's (empty passes
    /// included on both sides). The first pass is not a change.
    pub combiner_changes: u64,
    /// As `combiner_changes`, but only passes that ran at least one body count,
    /// and each is compared with the previous such pass.
    pub combiner_changes_nonempty: u64,
    /// Cap (pops) of the most recent pass; `u64::MAX` for locks without one.
    pub last_cap: u64,
    /// Passes that ran more bodies than their cap; always 0 for a sound lock.
    pub cap_violations: u64,
    /// Largest number of bodies in one pass.
    pub max_bodies: u64,
    /// `bodies_hist[min(bodies, 65)]` = passes with that many bodies.
    pub bodies_hist: Vec<u64>,
}

impl PassStats {
    pub fn bodies_per_pass(&self) -> Option<f64> {
        (self.passes > 0).then(|| self.bodies as f64 / self.passes as f64)
    }

    /// Combiner changes per 1,000 bodies (every pass, empty included).
    pub fn changes_per_1k_bodies(&self) -> Option<f64> {
        (self.bodies > 0).then(|| self.combiner_changes as f64 * 1000.0 / self.bodies as f64)
    }
}

#[derive(Debug)]
pub(crate) struct PassRecorder {
    stats: PassStats,
    last: usize,
    last_nonempty: usize,
}

impl PassRecorder {
    pub(crate) fn new() -> Self {
        Self {
            stats: PassStats {
                passes: 0,
                empty_passes: 0,
                bodies: 0,
                combiner_changes: 0,
                combiner_changes_nonempty: 0,
                last_cap: 0,
                cap_violations: 0,
                max_bodies: 0,
                bodies_hist: vec![0; HIST_BUCKETS],
            },
            last: 0,
            last_nonempty: 0,
        }
    }

    /// Record one pass run by `combiner` (a nonzero per-thread identity) that
    /// ran `bodies` delegates under a cap of `cap` pops.
    pub(crate) fn record(&mut self, combiner: usize, bodies: usize, cap: usize) {
        debug_assert_ne!(combiner, 0);
        let stats = &mut self.stats;
        stats.passes += 1;
        stats.last_cap = cap as u64;
        stats.bodies += bodies as u64;
        stats.max_bodies = stats.max_bodies.max(bodies as u64);
        stats.bodies_hist[bodies.min(HIST_BUCKETS - 1)] += 1;
        if bodies > cap {
            stats.cap_violations += 1;
        }
        if self.last != 0 && self.last != combiner {
            stats.combiner_changes += 1;
        }
        self.last = combiner;
        if bodies == 0 {
            stats.empty_passes += 1;
        } else {
            if self.last_nonempty != 0 && self.last_nonempty != combiner {
                stats.combiner_changes_nonempty += 1;
            }
            self.last_nonempty = combiner;
        }
    }

    pub(crate) fn snapshot(&self) -> PassStats {
        self.stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_counts_changes_empty_passes_and_cap_violations() {
        let mut recorder = PassRecorder::new();
        recorder.record(1, 3, 4); // first pass: no change
        recorder.record(1, 0, 4); // same combiner, empty
        recorder.record(2, 0, 4); // change (all), empty: not a non-empty change
        recorder.record(1, 2, 4); // change (all); non-empty vs last non-empty (1): none
        recorder.record(2, 5, 4); // change in both; violates cap 4
        let stats = recorder.snapshot();
        assert_eq!(stats.passes, 5);
        assert_eq!(stats.empty_passes, 2);
        assert_eq!(stats.bodies, 10);
        assert_eq!(stats.combiner_changes, 3);
        assert_eq!(stats.combiner_changes_nonempty, 1);
        assert_eq!(stats.cap_violations, 1);
        assert_eq!(stats.max_bodies, 5);
        assert_eq!(stats.bodies_hist[0], 2);
        assert_eq!(stats.bodies_hist[5], 1);
        assert_eq!(stats.bodies_per_pass(), Some(2.0));
        assert_eq!(stats.changes_per_1k_bodies(), Some(300.0));
    }
}
