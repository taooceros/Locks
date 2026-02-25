# TODO: Road to Conference Submission

Status legend: `[ ]` not started, `[~]` in progress, `[x]` done

---

## Phase 0: Foundation & Cleanup

- [x] **Implement Jain's Fairness Index computation.**
  *(Done: `a3131ac` — JFI computed from hold_time in `finish_benchmark()`.)*

- [x] **Add per-thread normalized share output (U-SCL style).**
  *(Done: `a3131ac` — `normalized_share` field in Records, printed per run.)*

- [ ] **Add response time CDF output.**
  Currently latencies are collected as `Vec<u64>`. Add percentile computation
  (p50, p95, p99, p99.9) and CDF data export. Split by combiner vs. waiter
  role. This is critical for tail-latency analysis.

- [ ] **Unify response time tracking across all DLock2 variants.**
  Verify that `--stat-response-time` works correctly for FC, FC-Ban, CC,
  CC-Ban, FC-PQ, FC-SL, DSM, Mutex, SpinLock, U-SCL. Some variants may not
  properly split combiner vs. waiter latency — audit and fix.

- [ ] **Clean up experiment.nu script.**
  Enable the currently-commented-out benchmarks (fetch-and-multiply, queue,
  priority-queue). Create separate scripts or flags for:
  - Quick smoke test (2 configs, short duration)
  - Full experiment suite (all configs, 15s each)
  - Response time focused (fewer configs, --stat-response-time)

- [x] **Stabilize DSMSynch implementation.**
  *(Done: `dfc261e` — multi-threaded correctness tests added for all DLock2
  variants including DSM at 2/4/8 threads.)*

---

## Phase 1: Fairness-Performance Tradeoff Validation (KEY EXPERIMENT)

This validates Contribution #3: delegation breaks the traditional
fairness-performance tradeoff because shared data stays in the combiner's
L1 regardless of serving order.

- [ ] **Implement CFL baseline (required comparison).**
  CFL (Manglik & Kim, PPoPP'24) is the closest related work for usage-fair
  traditional locks. Implement CFL-MCS in the DLock2 framework (or as a
  standalone wrapped baseline). This is NOT optional — CFL is the direct
  comparison point for our fairness-is-free argument.

- [x] **Implement MCS lock in DLock2 framework.**
  *(Done: `4d57e13` + `8e82f36` — MCS added as `DLock2Wrapper<RawMcsLock>`,
  uses per-lock ThreadLocal for queue nodes.)*

- [ ] **Run the tradeoff experiment.**
  - Workload: proportional counter with 3:1 CS ratio, 8/16/32 threads
  - Locks: FC, FC-PQ, FC-Ban, MCS, CFL, SpinLock, Mutex
  - Measure: throughput AND JFI for each
  - Key plot: scatter plot of (throughput, JFI) showing Pareto frontier
  - **Expected result:** FC→FC-PQ throughput gap is small (<10%?), while
    MCS→CFL throughput gap is large, because CFL pays cross-core migration
    on every fair handoff but FC-PQ doesn't.

- [ ] **Cache miss validation.**
  Run `perf stat` on FC vs FC-PQ vs MCS vs CFL, collecting L1/LLC misses.
  FC and FC-PQ should have similar shared-data miss rates (data stays in
  combiner's L1). CFL should show higher shared-data misses due to
  cross-core migration on handoff.

---

## Phase 2: Combiner Response-Time Study

- [ ] **Characterize combiner time distribution across thread counts.**
  Run FC, CC, FC-Ban, CC-Ban, FC-PQ at 4, 8, 16, 32, 64 threads. For each:
  measure what fraction of wall-clock time each thread spends as combiner.
  Produce box plots.

- [ ] **Implement tail-as-combiner selection for FC.**
  In `dlock2/fc/lock.rs`, modify combiner election so the tail of the thread
  list (last node executed) becomes the next combiner. Measure impact on
  combiner response-time variance.

- [ ] **Measure combiner vs. waiter response time separately.**
  For each lock variant, produce split CDFs. The key claim: unfair locks have
  bimodal response-time distributions; fair locks have more uniform ones.

- [ ] **Quantify combiner penalty scaling.**
  Plot: combiner response time / average waiter response time vs. thread
  count. Show this ratio grows with concurrency for unfair variants, stays
  bounded for fair variants.

---

## Phase 3: Additional Baselines

- [ ] **Add Ticket Lock to Rust benchmarks.**
  Already exists in C (`c/ticket/ticket.c`). Port to DLock2 framework or wrap
  via FFI. Simple FIFO lock, good baseline.

- [ ] **Implement CLH lock in DLock2 framework.**
  Similar to MCS but cache-friendlier on some architectures. Provides another
  acquisition-fair baseline. Lower priority than MCS.

- [ ] **Benchmark pthread_mutex with PTHREAD_MUTEX_ADAPTIVE_NP.**
  Linux-specific adaptive mutex that spins briefly before blocking. Common
  real-world baseline.

---

## Phase 4: Optimizations from Related Work

Concrete code tasks inspired by CFL, ShflLock, Syncord, TCLocks
(see RESEARCH_PLAN.md Section 4.6).

- [x] **Newcomer usage initialization (from CFL).**
  *(Done: `8403d71` — combiner tracks `total_usage / total_served`, newcomers
  with usage=0 initialized to running average in both FCPQ and FCSL.)*

- [ ] **Starvation counter (from ShflLock).**
  Add a counter to each FC-PQ node tracking how many combining passes it has
  been in the queue without being served. After K passes, clamp usage to
  current minimum. Guarantees bounded wait even under adversarial arrivals.

- [ ] **Prefetch next waiter's input (from TCLocks).**
  In FC-PQ's combining loop, after popping the heap root, issue a prefetch
  for the *next* root's data pointer before executing the current CS. Hides
  memory latency for loading the next waiter's input. Measure latency
  improvement with `perf stat`.

- [ ] **(Optional) NUMA-aware tie-breaking (from CFL).**
  Among threads with similar usage (within epsilon), prefer the one on the
  same NUMA node as the combiner. Only relevant on multi-socket machines.
  Requires reading NUMA topology at init time.

- [ ] **(Optional) Hybrid admission control (from Syncord).**
  Add a "soft ban" to FC-PQ: if a thread's usage exceeds 2x the average,
  delay its re-insertion into the PQ by one combining pass. Combines strict
  enforcement of banning with work-conservation of priority scheduling.

---

## Phase 5: End-to-End Applications

### A. Concurrent Hash Map

- [ ] **Implement lock-protected hash map benchmark.**
  - Separate-chaining hash map, single global lock (delegation-friendly)
  - Operations: get (short CS), put (medium CS), scan/range-query (long CS)
  - YCSB-like workload generator with configurable read/write/scan ratio
  - Thread groups: "lookup threads" (get-heavy) vs "scan threads" (scan-heavy)

- [ ] **Measure per-operation-type latency.**
  Track response time separately for get, put, and scan operations. The key
  result: under unfair delegation, get latency degrades when scan threads
  monopolize; fair variants bound get tail latency.

- [ ] **Compare against per-bucket fine-grained locking.**
  Show that delegation (even fair) can outperform fine-grained locking on
  throughput while maintaining fairness — the best of both worlds.

### B. Concurrent Priority Queue / Task Scheduler

- [ ] **Implement mixed-operation priority queue benchmark.**
  - Operations: insert (variable batch size = variable CS), extract-min
    (short CS)
  - Thread groups: "producers" (bulk insert, long CS) vs "consumers"
    (extract-min, short CS)
  - Measures: consumer starvation under unfair vs. fair locks

- [ ] **Demonstrate natural scheduling property of FC-PQ/FC-SL.**
  Show that priority-based combining naturally gives lower latency to threads
  that have consumed less lock time — a property that banning cannot provide.

### C. (Optional) Log / Write-Ahead Log

- [ ] **Implement shared append-only log benchmark.**
  - Small appends (short CS) vs. batch flushes (long CS)
  - Relevant to database and storage systems
  - Shows fairness under realistic mixed I/O pattern

---

## Phase 6: Multi-Machine & NUMA Evaluation

- [ ] **Obtain access to a NUMA machine.**
  Cloudlab c6525-100g (AMD EPYC 7543, 32 cores, 2 NUMA nodes) or similar.
  Alternatively, any dual-socket Intel/AMD system.

- [ ] **Run full micro-benchmark suite on NUMA machine.**
  Repeat all experiments from Phase 0-1 on the second machine. Document
  topology with `lscpu` and `numactl --hardware`.

- [ ] **Test NUMA-aware thread placement.**
  Run with threads spread across NUMA nodes vs. packed on one node. Measure
  whether banning/priority mechanisms interact with NUMA topology.

- [ ] **Consider H-Synch extension.**
  If NUMA effects are significant, implement NUMA-aware fair combining
  (per-NUMA-node queues with fair inter-node arbitration). This could be a
  contribution or future work depending on time.

---

## Phase 7: Overhead Analysis

- [ ] **Run perf stat profiles for all lock variants.**
  Extend `profile.nu` to cover all DLock2 variants. Collect:
  - Cache references/misses (L1, L2, LLC)
  - dTLB load/store misses
  - CPU migrations
  - Branch mispredictions
  - Instructions per cycle (IPC)

- [ ] **Quantify fairness mechanism overhead.**
  Measure: what is the throughput delta between FC and FC-Ban? Between CC and
  CC-Ban? Between FC and FC-PQ? Express as percentage overhead. This directly
  supports the "fairness is cheap in delegation" claim.

- [ ] **Microbenchmark priority queue operations.**
  Isolated benchmark of BinaryHeap vs. BTreeSet vs. SkipList for the
  insert/pop pattern used by FC-PQ/FC-SL. Determines which priority structure
  is cheapest for small N (typical combining pass size).

- [ ] **Measure banning idle time.**
  How much time do banned threads spend spinning/sleeping? Is this wasted CPU
  or does the OS scheduler reclaim it? Measure with and without `sched_yield`
  during ban wait.

---

## Phase 8: Writing

- [ ] **Draft Section 3 (Design).**
  - Formal usage-fairness definition
  - Banning algorithm with pseudocode
  - Priority-based combining with pseudocode
  - Combiner selection strategy
  - Implementation notes (Rust, cache padding, memory ordering)

- [ ] **Draft Section 4 (Evaluation).**
  - Experimental setup (machines, methodology)
  - **Fairness-performance tradeoff figure** (the central result from Phase 1)
  - Micro-benchmark throughput graphs
  - Fairness analysis (JFI tables, per-thread bar charts)
  - Response time CDFs (combiner vs. waiter)
  - Combiner penalty characterization
  - End-to-end application results
  - Overhead breakdown tables

- [ ] **Draft Section 1-2 (Intro + Background).**
  - Motivation example (CCSynch unfairness with heterogeneous CS)
  - Combiner penalty observation figure
  - **"Delegation breaks the tradeoff" argument** as a key selling point
  - Prior work positioning table

- [ ] **Draft Section 5-7 (Discussion + Related Work + Conclusion).**
  - CFL/ShflLock: off-path vs on-path scheduling distinction
  - Syncord: validates banning, we provide concrete algorithms
  - TCLocks: complementary (transparency vs fairness)
  - U-SCL: delegation preserves scheduling point
  - Async/coroutine combiner vision as future work
  - NUMA extension discussion

- [ ] **Produce publication-quality figures.**
  Replace current SVG plots with consistent, publication-ready figures.
  Use matplotlib/pgfplots with consistent style, proper axis labels,
  legends, and font sizes suitable for 2-column format.

- [ ] **Internal review round.**
  Circulate draft to co-authors / advisor. Iterate.

---

## Phase 9: Stretch Goals (if time permits)

- [ ] **Async/coroutine combiner prototype.**
  Use Rust async/await to implement a combiner that can yield its
  non-critical section to a waiting thread. Even a proof-of-concept
  demonstrating the concept would significantly strengthen the paper's
  vision section.

- [ ] **Transparent delegation integration.**
  Show that fair delegation locks can be used with TCLocks-style transparent
  API. This would eliminate the "requires code refactoring" objection.

- [ ] **Comparison against actual CFL / U-SCL / TCLocks codebases.**
  Instead of our re-implementations, benchmark against the original authors'
  code. Strengthens credibility of comparison.

- [ ] **Dynamic banning adaptation.**
  Instead of fixed `penalty = cs * n_threads`, explore adaptive penalty that
  adjusts based on observed fairness (feedback loop). Potentially better
  convergence to fair state.

- [ ] **Multiple-lock scenario.**
  Benchmark with 2+ locks protecting different data structures. Show that
  per-lock fairness composes correctly when threads hold different lock
  combinations.

---

## Key Dependencies & Blockers

| Item | Blocks | Notes |
|------|--------|-------|
| ~~JFI implementation~~ | ~~Phase 1, Phase 8~~ | Done (`a3131ac`) |
| CFL baseline implementation | Phase 1 | Critical for the central claim |
| ~~MCS baseline implementation~~ | ~~Phase 1~~ | Done (`4d57e13`) |
| Response time CDF tooling | Phase 2, Phase 5 | Reusable across all experiments |
| NUMA machine access | Phase 6 | Apply for Cloudlab allocation early |
| End-to-end app design | Phase 5, Phase 8 | Needs agreement on which apps are most compelling |
| Combiner penalty data | Phase 8 (intro figures) | The motivating figure needs this data |
| Scope the O(C_max) claim | Phase 8 | See note below |

---

## Priority Order (what to do first)

1. **Phase 0** — metrics and tooling (JFI, CDF, response time audit)
2. **Phase 1** — fairness-performance tradeoff (requires CFL + MCS baselines)
3. **Phase 2** — combiner response-time study
4. **Phase 4** — newcomer init + starvation counter (quick wins, strengthen fairness story)
5. **Phase 5** — end-to-end applications (at least hash map)
6. **Phase 3** — remaining baselines (ticket, CLH, adaptive mutex)
7. **Phase 6** — NUMA machine experiments
8. **Phase 7** — overhead analysis (perf stat)
9. **Phase 8** — writing

Phase 1 is the most important because it validates the paper's strongest
claim. If FC→FC-PQ throughput gap is small and MCS→CFL gap is large, the
story writes itself. If not, we need to rethink the framing.

**Framing note (2025-02-25):** The O(C_max) usage-fairness bound is real
but must be scoped carefully. It bounds *cumulative lock-holding time* gap,
NOT response time. The PQ maintenance adds O(N log N) work per combining
pass vs CFL's O(N) queue traversal — the asymptotic cost is worse, but
over L1-hot sequential data vs remote CAS. The paper claim should be:
"adding fairness to delegation is cheaper than adding fairness to
traditional locks" — compare the FC→FC-PQ delta vs MCS→CFL delta, not
FC-PQ vs CFL directly. They solve fairness in different paradigms.

---

## Decision Points

1. **CFL implementation strategy?** Re-implement CFL-MCS in Rust within
   DLock2, or build/wrap the original C code? Re-implementation is cleaner
   for apples-to-apples comparison. Original code is more credible but
   harder to integrate.

2. **Which end-to-end apps?** Hash map is the safest choice (well-understood,
   easy to argue relevance). Priority queue is natural given FC-PQ. Choose at
   least one, ideally two.

3. **PPoPP vs. EuroSys?** If the tradeoff experiment + one end-to-end app
   are solid by Aug 2026, target PPoPP. If more time is needed, fall back
   to EuroSys (Oct 2026).

4. **Include async/coroutine prototype?** High reward but high risk. Decide
   after Phase 2 — if core evaluation is solid, it's better as future work.

5. **Rust-only or also C?** DLock2 (Rust) is the primary framework. C
   implementations exist but are less complete. Argue Rust's zero-cost
   abstraction makes it equivalent in performance. If reviewers push back,
   the C FC-Ban implementation can serve as validation.
