//! Measurement primitives copied verbatim from `crates/coro_delegation`
//! (`lock::cycles`, `workload::{spin_cycles, jain, estimate_tsc_hz,
//! XorShift64}`, `stats::{Histogram, LatencySummary}`), so that both runtimes
//! spin, sample keys, bucket latencies and compute quantiles with identical
//! code. This crate deliberately does not depend on `coro_delegation`; if the
//! originals change, update these copies.

use std::time::{Duration, Instant};

use serde::Serialize;

/// `rdtscp`-based cycle counter.
#[inline]
pub fn cycles() -> u64 {
    let mut aux = 0u32;
    // SAFETY: rdtscp is available on every x86-64 target this crate supports.
    unsafe { core::arch::x86_64::__rdtscp(&mut aux) }
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

pub struct XorShift64(pub u64);

impl XorShift64 {
    #[inline]
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

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

/// Summary of a [`Histogram`] in TSC cycles.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct LatencySummary {
    pub count: u64,
    pub mean: f64,
    pub p50: u64,
    pub p99: u64,
    pub max: u64,
}
