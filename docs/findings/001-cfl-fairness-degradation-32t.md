# Finding 001: CFL Fairness Degradation at 32 Threads

**Date:** 2026-03-02
**Status:** Confirmed (code audit + empirical)
**Component:** CFL baseline (`crates/libdlock/src/dlock2/cfl.rs`, `c/cfl/cfl.c`)
**Severity:** Algorithmic limitation (not a porting bug)
**Machine:** saturn (128T Intel Xeon Gold 6438M, 2-socket Sapphire Rapids)

## Observation

CFL sometimes exhibits poor Jain's Fairness Index (JFI) at 32 threads. The
degradation is intermittent — some runs achieve JFI > 0.99 while others show
significantly worse fairness.

## Root Cause Analysis

The Rust port was verified line-by-line against the C reference implementation
(`c/cfl/cfl.c`). **No porting bugs were found.** The fairness degradation is
inherent to the CFL algorithm.

### Primary Cause: Cross-Configuration Counter Pollution

CFL uses **process-global, never-reset cumulative counters** for fairness:

```c
unsigned long runtime_checker_core[256];  // per-core accumulated runtime
unsigned long runtime_checker_node[16];   // per-NUMA-node accumulated runtime
```

When the benchmark runs CFL at ascending thread counts within a single process
(e.g., `-t 4,8,16,32` as in group1), the counters from smaller thread counts
**poison** the fairness decisions at larger thread counts:

- **After 4T run:** `rt_core[0..3]` = large, `rt_core[4..31]` = 0
- **After 8T run:** `rt_core[0..7]` = large, `rt_core[8..31]` = 0
- **After 16T run:** `rt_core[0..15]` = large, `rt_core[16..31]` = 0
- **At 32T:** cores 0-15 have massive stale values, cores 16-31 start at zero

The shuffle skip logic then creates **systematic starvation**:

```c
standard = runtime_checker_node[nid] / 16;  // dominated by cores 0-15
if (runtime_checker_core[curr->cid] >= standard) {
    prev = curr;
    goto check;  // SKIP — this core "had enough"
}
```

- Cores 0-15: `rt_core >> standard` → **always skipped** → threads starved
- Cores 16-31: `rt_core ≈ 0 << standard` → **always promoted** → 3-4x fair share

The fast-path CAS compounds this — with `allowed_node == 100`, threads bypass
the queue entirely via `smp_cas(&impl->locked_no_stealing, 0, 1)`, and only
threads on promoted cores win the race.

### Secondary: Counter Saturation (Long Single Runs)

Even without multi-config interference, long runs at a single thread count show
mild degradation. The `need_switch()` threshold of 100,000 TSC cycles becomes
insignificant relative to accumulated counters (~10^12 cycles after 60s),
causing it to always return "balanced" and disabling NUMA-level fairness.

### Other Contributing Factors

- **`keep_lock_local()` has empty body** in both C and Rust — the PRNG result
  is discarded, so the shuffling frequency control is a no-op
- **Single-socket 32T** disables NUMA-level fairness (all threads on one node)

## Implications for FC-PQ

This is a **strength** of FC-PQ's design:

- FC-PQ uses **per-lock, per-request priority** (binary heap keyed on cumulative
  service), not process-global accumulated counters. No cross-config pollution.
- FC-PQ's combiner always processes the priority queue — no fast-path bypass
  that skips fairness.
- FC-PQ's O(C_max) bound holds regardless of thread count, run duration, or
  prior execution history.

This finding strengthens the paper's argument: *"CFL's global vLHT counters
create fragile fairness that breaks under realistic deployment conditions
(varying thread counts, long runs). Delegation's per-request priority scheduling
is robust by construction."*

## Implications for Benchmarking Methodology

**CFL results in multi-config benchmark suites are unreliable.** Any experiment
that runs CFL at multiple thread counts in a single process (standard practice)
will produce poisoned fairness data at larger thread counts. Options:

1. **Spawn a separate process per thread count** (recommended for paper)
2. Reset CFL global counters between configs (requires modifying CFL)
3. Run thread counts in descending order (32T first, then 16T, etc.) — but
   this only shifts the problem

This is NOT a concern for FC-PQ, MCS, or any lock with per-instance state.

## Empirical Validation

### Experiment A: Cross-Configuration Pollution (group1 scenario)

**Reproduced JFI as low as 0.29.** Single process, ascending thread counts
(`-t 4,8,16,32`), CS=1000,3000, 15s, 3 trials. This simulates group1's
iteration pattern where CFL runs at 4T → 8T → 16T before reaching 32T.

#### CFL at 32T (after 4T/8T/16T poisoned counters)

| Trial | JFI | Starved Threads | Worst Share | Best Share |
|-------|---------|-----------------|-------------|------------|
| T0 | **0.2938** | 16 threads at 0.0000 | 0.0000 | 3.7075 |
| T1 | **0.3636** | 16 threads at 0.0000 | 0.0000 | 3.0973 |
| T2 | **0.3999** | 16 threads at 0.0000 | 0.0000 | 2.8732 |

**Half the threads (cores 0-15) got literally zero operations.** Their stale
`runtime_checker_core` values from prior 4T/8T/16T runs caused the shuffler to
permanently skip them. Threads on cores 16-31 (fresh counters) got 3-4x fair share.

#### FC-PQ at 32T (same process, same poisoned state)

| Trial | JFI |
|-------|---------|
| T0 | 0.9994 |
| T1 | 0.9994 |
| T2 | 0.9994 |

FC-PQ is completely immune — it uses per-lock, per-request priority, not global counters.

### Experiment B: Duration Sweep (isolated, no prior runs)

Experiment: 32T, CS=1000,3000 (3:1 heterogeneous), 5 trials per duration.
Ran on saturn, 2026-03-02. Data in `visualization/output/{CFL,FC_PQ_BHeap}/cfl-duration-sweep-*.arrow`.

### CFL JFI by Duration (5 trials)

| Duration | T1 | T2 | T3 | T4 | T5 | Mean | Std | Min |
|----------|--------|--------|--------|--------|--------|--------|--------|--------|
| 5s | 0.9985 | 0.9986 | 0.9986 | 0.9986 | 0.9986 | 0.9986 | 0.0000 | 0.9985 |
| 15s | 0.9987 | 0.9987 | 0.9986 | 0.9986 | 0.9987 | 0.9987 | 0.0000 | 0.9986 |
| 30s | 0.9988 | 0.9987 | 0.9983 | 0.9986 | 0.9984 | 0.9986 | 0.0002 | 0.9983 |
| **60s** | 0.9988 | 0.9985 | 0.9987 | 0.9986 | **0.9943** | 0.9978 | **0.0020** | **0.9943** |

### FC-PQ JFI by Duration (control)

| Duration | T1 | T2 | T3 | T4 | T5 | Mean | Std |
|----------|--------|--------|--------|--------|--------|--------|--------|
| 5s | 0.9994 | 0.9994 | 0.9994 | 0.9994 | 0.9993 | 0.9994 | 0.0000 |
| 15s | 0.9994 | 0.9994 | 0.9994 | 0.9994 | 0.9994 | 0.9994 | 0.0000 |
| 30s | 0.9994 | 0.9994 | 0.9994 | 0.9994 | 0.9994 | 0.9994 | 0.0000 |
| 60s | 0.9994 | 0.9994 | 0.9994 | 0.9994 | 0.9994 | 0.9994 | 0.0000 |

### Worst-Case Per-Thread Shares (CFL outliers)

| Trial | Duration | Min Share | Max Share | Thread |
|-------|----------|-----------|-----------|--------|
| 30s T2 | 30s | **0.8875** | 1.0407 | thread 24 |
| 30s T4 | 30s | **0.9009** | 1.0429 | thread 2 |
| 60s T4 | 60s | **0.7372** | **1.1348** | thread 24 / thread 2 |

### Analysis

1. **Variance increases with duration**: CFL's JFI std dev: 0.0000 (5s) → 0.0002 (30s) → 0.0020 (60s).
   FC-PQ remains 0.0000 throughout.

2. **60s trial 4 is the smoking gun**: JFI=0.9943, one thread at 73.7% fair share,
   another at 113.5%. CFL also produced 13% higher throughput in that trial
   (60.4B vs ~53.3B) — the unfairness "freed" throughput by letting threads monopolize.

3. **30s already shows cracks**: Two of five trials had per-thread outliers below 0.91,
   though the aggregate JFI stayed above 0.998.

4. **The degradation is stochastic, not monotonic**: Not every long run degrades.
   The counter saturation creates a *probability* of unfairness that increases with
   duration, depending on timing and core placement.

5. **FC-PQ is invariant**: JFI=0.9994 across all 20 trials (4 durations × 5 trials),
   zero variance. Per-request priority scheduling is immune to cumulative counter drift.

### Throughput Context

CFL throughput is ~27% lower than FC-PQ in the fair regime (~890M vs ~1230M ops/s
at 32T). When CFL's fairness breaks (60s T4), its throughput jumps from ~890M to
~1006M ops/s — confirming that the unfairness "buys" throughput by letting some
threads monopolize, which is exactly the tradeoff CFL was designed to avoid.

## Verification

The Rust port was audited against the C reference on 2026-03-02. All control
flow, pointer manipulation, atomic orderings, union layouts, and PRNG behavior
match. The `keep_lock_local()` empty-body pattern is present in both and
appears to originate from the upstream fairnumas.c source.

## Reproduction Scripts

```bash
# Experiment A: Cross-configuration pollution (reproduces JFI ~0.3)
target/release/dlock d-lock2 -t 4,8,16,32 -l cfl,fc-pq-b-heap \
  counter-proportional --cs 1000,3000 --non-cs 0 -d 15 --trials 3 \
  --file-name cfl-group1-repro

# Experiment B: Duration sweep (isolated, reproduces variance increase)
for dur in 5 15 30 60; do
  target/release/dlock d-lock2 -t 32 -l cfl,fc-pq-b-heap \
    counter-proportional --cs 1000,3000 --non-cs 0 -d $dur --trials 5 \
    --file-name cfl-duration-sweep-${dur}s
done

# Analysis
python3 visualization/analyze_cfl_duration.py
```
