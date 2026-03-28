# Status Report: Usage-Fair Delegation Locks
**Date:** 2026-02-25 (updated)
**Target:** PPoPP 2027 (Aug 2026 deadline) / EuroSys 2027 fallback (Oct 2026)

Multi-agent code review of the entire project (architecture, algorithms,
evaluation, paper framing, applications). Findings organized by priority.

---

## Critical Bugs (Fix Before Any Experiments)

### ~~BUG-1: Arrow IPC files never `finish()`ed~~ [RESOLVED]
*(Fixed: `353f1ca` — `WriterMap` wrapper with `Drop` impl calls `finish()`
on all FileWriters when dropped.)*

### ~~BUG-2: Non-atomic writes to AtomicPtr~~ [RESOLVED]
*(Fixed: `353f1ca` — replaced raw pointer dereference with atomic load/store.)*

### ~~BUG-3: `UnsafeCell` instead of `SyncUnsafeCell`~~ [RESOLVED]
*(Fixed: `353f1ca` — `age` field changed to `SyncUnsafeCell` in FC and FC-Ban.)*

### BUG-4: `num_waiting_threads` overestimation [SEVERITY: CORRECTNESS]
**FC-Ban:** Counter incremented in `push_node()`, decremented only in
`clean_unactive_node()` (every 500 passes). Between cleanups, departed
threads inflate the count. With 32 threads / 10 active, penalty is 3.2x
too large.
**CC-Ban:** Counter is monotonically increasing — never decremented at all.
Counts total threads ever participated, not currently active.

**Fix (FC-Ban):** Decrement when `complete = true` is set inside `combine()`.
**Fix (CC-Ban):** Decrement when ban expires and thread re-joins queue, or
use a separate `currently_queued` atomic.

### ~~BUG-5: FC-SL `active` flag never set~~ [RESOLVED]
*(Fixed: `353f1ca` — active flag set/cleared properly in push_node/combine.)*

### ~~BUG-6: `PairingHeap` and `lockfree_queue` are `todo!()`~~ [RESOLVED]
*(Fixed: `353f1ca` — PairingHeap removed from CLI. lockfree_queue still
unimplemented but not exposed as a selectable option.)*

---

## Hard Blockers for Paper Submission

### BLOCK-1: Implement CFL-MCS Baseline [Est: 1-2 weeks]
The paper's central claim ("FC->FC-PQ throughput gap is smaller than
MCS->CFL throughput gap") is **untestable without CFL**. This is the
single most important missing piece.

**Strategy:** Re-implement CFL-MCS in Rust inside DLock2 framework.
CFL = MCS queue + per-thread vLHT accounting + Lock Scheduler thread
that reorders queue off-path. If original code is on GitHub, benchmark
both (ours vs. theirs) for credibility.

### ~~BLOCK-2: Implement MCS Lock~~ [RESOLVED]
*(Done: `4d57e13` + `8e82f36` — MCS added as `DLock2Wrapper<RawMcsLock>`.)*

### ~~BLOCK-3: Fix Newcomer Usage Initialization~~ [RESOLVED]
*(Done: `8403d71` — Combiner tracks running average, newcomers initialized.)*

### ~~BLOCK-4: Write DLock2 Correctness Tests~~ [RESOLVED]
*(Done: `dfc261e` — Multi-threaded counter tests for all variants at 2/4/8
threads. Fairness validation and ban-specific tests still TODO.)*

### ~~BLOCK-5: JFI + Per-Thread Normalized Share~~ [RESOLVED]
*(Done: `a3131ac` — JFI and normalized_share computed in `finish_benchmark()`,
added to Records struct, printed per run.)*

---

## Key Theoretical Result

**FC-PQ has an N-independent fairness bound (provable):**

> For any two threads i, j at any point during execution:
> `|U_i - U_j| <= C_max`
> where C_max is the maximum single CS duration.

**Proof sketch:** PQ invariant ensures minimum-usage thread is served.
After serving, its usage becomes `U_min + c_k` where `c_k <= C_max`.
Next minimum is >= U_min. By induction, no thread advances more than
C_max ahead of current minimum.

This is **strictly stronger** than CFL's O(N * C_max) per pass or
U-SCL's settling time. Newcomer initialization fix landed (`8403d71`),
so the bound now holds in the implementation.

**Scope this claim carefully (2025-02-25 review):**
- The bound is on *cumulative lock-holding time*, NOT response time.
  Response time includes waiting for the combiner's full pass (up to H ops).
- PQ maintenance is O(N log N) per combining pass vs CFL's O(N) traversal.
  The asymptotic cost is worse, but over L1-hot sequential data vs remote CAS.
- The paper should frame as: "cost of adding fairness to delegation is
  smaller than cost of adding fairness to traditional locks" — compare
  FC→FC-PQ delta vs MCS→CFL delta. Not FC-PQ vs CFL directly.

**Starvation freedom:** Also provable — every thread eventually reaches
U_min and gets served (requires starvation counter for adversarial
arrivals).

---

## Paper Framing Improvements

### Consolidate to 3 Contributions (from 5)
1. **Formal framework:** Usage-fairness definition for delegation;
   acquisition-fairness is insufficient; O(C_max) bound for FC-PQ.
2. **Four algorithm variants:** FC-Ban, CC-Ban, FC-PQ, FC-SL with
   different tradeoffs (work-conservation, strictness, overhead).
3. **Key insight + validation:** Delegation enables O(log N) L1-hot
   fairness scheduling, breaking the traditional tradeoff. Validated:
   FC-PQ gap from FC << CFL gap from MCS.

### Front-Load CFL Differentiation
Must appear by paragraph 2 of the intro, not buried in Section 4.
Suggested abstract sentence:
> "Unlike CFL, which schedules fairness off the critical path via O(N)
> remote cache reads over a distributed queue, delegation locks enable
> O(log N) on-path fairness scheduling over L1-hot data structures."

Also explicitly note: CFL's Lock Scheduler requires a traversable linked
queue that FC's publication list doesn't provide — CFL cannot be trivially
applied to delegation locks.

### Add Missing Related Work
Reviewers will notice the absence of:
- **Lock Cohorting** (Dice, Marathe, Shavit, PPoPP'12) — cohort holder
  is conceptually similar to delegation
- **CNA** (Dice & Kogan, PPoPP'19) — NUMA-aware MCS, relevant since
  paper discusses NUMA effects
- **H-Synch** (Fatourou & Kallimanis) — hierarchical combining,
  mentioned in Discussion but not Related Work

### Scope the Central Claim
"Delegation breaks the tradeoff" needs explicit boundaries:
- Holds under moderate-to-high contention, heterogeneous CS, 10-1000ns range
- Very long CS: serialization dominates, all locks perform similarly
- Low contention: PQ overhead may dominate for short CS
- NUMA: waiter input/output still requires cross-socket transfer
  (shared data stays in L1, but delegate args don't)
- Homogeneous CS: all delegation locks are already usage-fair; mechanisms
  add overhead with zero benefit

### Restructure Paper
```
1. Introduction (2 pages)
   - Problems A and B with motivating numbers
   - Core insight: combiner as natural scheduling point
   - CFL differentiation in paragraph 2
   - 3 contributions

2. Background & Motivation (2 pages)
   - FC/CCSynch/RCL mechanics
   - Usage-fairness definition (formal)
   - Quantitative motivation figures

3. Key Insight: Why Delegation Enables Cheap Fairness (0.75 pages)
   - Cache-migration argument
   - Why traditional locks cannot achieve this

4. Design (3 pages)
   - Banning + correctness sketch / fairness bound
   - Priority-based combining + newcomer init
   - Combiner selection optimization

5. Evaluation (3.5 pages)

6. Related Work (1.25 pages)

7. Conclusion (0.5 pages)
```

---

## Evaluation Improvements

### Statistical Rigor
- [x] **Multiple trials (>=3)** — *(Done: `a3131ac` — `--trials N` flag,
  trial index tracked in Records.)*
- [x] **Warmup phase** — *(Done: `a3131ac` — `--warmup` flag, threads run
  but `warmup_done` gate suppresses stat accumulation.)*
- [ ] **CPU frequency pinning** — add `cpupower frequency-set -g
  performance` to experiment scripts. Standard practice.
- [ ] **Environment capture** — log `lscpu`, `numactl --hardware`,
  kernel version, CPU governor at experiment start.

### Response Time Infrastructure
- [ ] **Add percentile computation** (p50, p95, p99, p99.9) — raw data
  exists in `combiner_latency` and `waiter_latency` Vecs but percentiles
  are never computed.
- [ ] **Audit combiner/waiter detection** across all variants —
  `is_combiner` check uses `current().id() == thread_id` which may not
  be correct for all lock types (especially DSM).

### The Central Experiment (Phase 1)
1. Implement MCS
2. Implement CFL-MCS
3. Add JFI post-processing
4. Run proportional_counter: cs=[1000,3000] at 8/16/32 threads, 15s
5. Scatter plot: x=throughput, y=JFI per (lock, thread_count)
6. `perf stat`: validate L1-miss counts (FC ~ FC-PQ << CFL)
7. **Run this BEFORE investing in writing** — validates or falsifies
   the core thesis

### NUMA
- [ ] **Apply for Cloudlab c6525-100g NOW** — allocations take days/weeks.
  This is on the critical path for EuroSys submission.
- [ ] Run full benchmark suite on NUMA machine.
- [ ] Test whether fair ordering increases cross-socket traffic.

---

## Algorithm Improvements (Priority Order)

### 1. Newcomer Usage Initialization [CRITICAL — blocks formal bound]
Initialize to running average, not 0. Combiner tracks `total_usage` and
`total_served`. New thread: `node.usage = total_usage / total_served`.
**Est: 1 day.**

### 2. Starvation Counter [HIGH — enables starvation-freedom proof]
Add `wait_count: u32` to `UsageNode`. Increment each combining pass.
After K=10 passes, set `usage = current_min - 1` (force to front).
Guarantees bounded wait under adversarial arrivals.
**Est: 1 day.**

### 3. Time-Based Combining Budget [HIGH — addresses Problem B]
Replace fixed H=64 with TSC budget: `while elapsed < budget_tsc`.
More principled than arbitrary batch size. Enables claim: "FC-PQ with
budget achieves O(budget) combiner latency regardless of CS
heterogeneity."
**Est: 1 day.**

### 4. Adaptive Banning [MEDIUM — novel contribution]
Replace fixed penalty `cs * N` with:
`penalty = cs * N * (U_actual / U_expected)^alpha`
PI-controller style feedback loop. Faster fairness convergence, less
throughput loss. Neither U-SCL nor CFL does this.
**Est: 2 days.**

### 5. Prefetch Next Waiter [MEDIUM — measurable latency reduction]
After `job_queue.pop()`, prefetch next heap root's data pointer while
executing current CS. Expected 5-15% latency reduction for short CS.
Measurable via `perf stat` L1-miss delta.
**Est: 1 day.**

### 6. NUMA-Aware Tie-Breaking [LOW — defer to Phase 6]
Among threads with similar usage (within epsilon), prefer same-NUMA-node
thread. Only implement after NUMA experiments show significant effects.

### 7. Async/Coroutine Combiner [DEFER — future work]
Genuinely novel idea but high implementation risk. Include as pseudocode
in Discussion section. Don't implement for this paper.

---

## Application Benchmarks

### Priority 1: Concurrent Hash Map + YCSB [Est: 2-3 weeks]
- Separate-chaining hash map, single global lock
- Operations: get (short CS), put (medium CS), scan/range-query (long CS)
- Thread groups: "lookup threads" (get-heavy) vs "scan threads"
- Key result: lookup p99 drops with FC-PQ while throughput stays within 5%
- Industry-standard benchmark; reviewers expect YCSB

### Priority 2: Task Queue with Heterogeneous Jobs [Est: 1-2 weeks]
- Heavy workers (long CS batch jobs) vs light workers (short CS tasks)
- Most relatable to cloud/RPC audiences
- Directly connects to real thread-pool designs

### Expand CS Ratios
Current plan only tests 3:1 ratio. Real workloads have 100:1+.
Test: **3:1, 10:1, 100:1** minimum. Show JFI degrades with ratio for
FC but stays flat for FC-PQ.

### Fix Existing Benchmarks
- [ ] `queue.rs` and `priority_queue.rs` use symmetric 50/50 push/pop
  with identical CS — does NOT demonstrate fairness. Need distinct thread
  groups.
- [ ] `lockfree_queue` is `todo!()` — implement or remove.

### Real-World Motivation (for Risk Q2)
Cite production delegation patterns:
- **RocksDB** write batch group commit (variable batch sizes per writer)
- **PostgreSQL** WAL group commit (variable wal_size per transaction)
- **Multi-tenant Redis/Memcached**: tenant A's SCAN monopolizes combining
  pass, violating tenant B's p99 SLA

---

## Code Quality Improvements

### High Priority
- [ ] **Factor out shared combiner election loop** — FC/FCBan/FCPQ/FCSL
  share ~80% identical outer loop. Bugs fixed in one will be missed in
  others. Use macro or generic combiner struct.
- [ ] **Replace `crossbeam_skiplist::SkipSet` in FC-SL** with `BTreeSet`
  — combiner-only sequential access doesn't benefit from lock-freedom.
  Wasted CAS overhead.
- [ ] **Replace `thread::current().id()` in FC-PQ** with node pointer
  address as tie-breaker. `std::thread::current()` may clone thread name
  string on every call.
- [ ] **Create workspace Cargo.toml** — `lib-dlock` and binary crate
  aren't in a workspace, so `cargo build` at root doesn't work. Unify
  dependency versions and feature flags.

### Medium Priority
- [ ] **Fix `panelty` typo** -> `penalty` in CC-Ban (throughout).
- [ ] **Cache-pad FC/FCBan node `active` field** — FC-PQ already does
  this with `CachePadded<AtomicBool>`, base FC nodes don't.
- [ ] **Add SAFETY comments** to all `unsafe` blocks.
- [ ] **Fix `aux` variable shadowing** in `fc_ban/lock.rs::combine()`.
- [ ] **Reduce nightly feature dependencies** — `sync_unsafe_cell` is
  stabilized since Rust 1.75, `thread_id_value` can be replaced with
  pointer address, `trait_alias` can use blanket impl.
- [ ] **Document `unsafe trait DLock2<I>` safety contract** or remove
  `unsafe` from the trait.
- [ ] **Fix `get_combine_time()` conditional compilation** — trait
  interface changes based on feature flag. Make method always present,
  returning `None` for non-combiner locks.

---

## Venue Strategy

| Venue | Fit | Deadline | Prerequisites |
|-------|-----|----------|---------------|
| **PPoPP 2027** | Best | ~Aug 2026 | CFL baseline, MCS, 1 app benchmark, NUMA experiment |
| **EuroSys 2027** | Strong | ~Oct 2026 | Above + stronger app story, ideally C impl |
| **ATC 2027** | Good | ~Jan 2027 | Deployment narrative |

**Primary target: PPoPP 2027.**

**Strongest reviewer objections to preempt:**
1. "CFL already solved this" -> front-load architectural distinction,
   show CFL can't apply to delegation
2. "Rust-only" -> consider C implementation of FC-PQ, cite Rust in
   Linux kernel / production systems
3. "Only helps under heterogeneous CS" -> explicitly scope, show
   heterogeneity is common in real workloads
4. "Delegation API is restrictive" -> cite TCLocks as orthogonal
   solution, list naturally delegation-friendly data structures

---

## Suggested Execution Order

```
Week 1-2:  Fix critical bugs (BUG-1 through BUG-6)
           Write DLock2 correctness tests
           Implement JFI + per-thread normalized share
           Apply for Cloudlab NUMA machine

Week 3-4:  Implement MCS lock
           Implement CFL-MCS baseline
           Fix newcomer usage initialization
           Add starvation counter

Week 5:    Run central experiment (Phase 1)
           Validate with perf stat cache miss data
           Add multiple trials + warmup + CPU pinning

Week 6-7:  Combiner response-time study (Phase 2)
           Time-based combining budget
           Adaptive banning

Week 8-10: Concurrent hash map benchmark (YCSB)
           Expand CS ratios (3:1, 10:1, 100:1)
           NUMA machine experiments

Week 11-12: Overhead analysis (perf stat profiles)
            Additional baselines (Ticket, CLH) if time

Week 13-15: Write paper (Design + Evaluation first)
            Publication-quality figures

Week 16:   Internal review + revision
```
