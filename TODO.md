# TODO: Road to Conference Submission

Status legend: `[ ]` not started, `[~]` in progress, `[x]` done

---

## Phase 0: Foundation & Cleanup

- [x] **Implement Jain's Fairness Index computation.**
  *(Done: `a3131ac` — JFI computed from hold_time in `finish_benchmark()`.)*

- [x] **Add per-thread normalized share output (U-SCL style).**
  *(Done: `a3131ac` — `normalized_share` field in Records, printed per run.)*

- [x] **Add response time CDF output.**
  *(Done: `566a303` — percentile computation (p50/p95/p99/p99.9) and CDF CSV
  export in `counter_common.rs`. Split by combiner/waiter role.)*

- [x] **Unify response time tracking across all DLock2 variants.**
  *(Done: `e1e0f99` — `report_response_times()` extracted as shared function,
  called from all four benchmark variants: counter, counter-array,
  fetch-and-multiply, queue/priority-queue.)*

- [x] **Clean up experiment.nu script.**
  *(Done: `2dc6882` — Rewritten with functions per experiment group (1-5, 8, 2b),
  smoke test, consistent parameters matching EXPERIMENT_PLAN.md. profile.nu
  updated to cover all 14 lock variants.)*

- [x] **Stabilize DSMSynch implementation.**
  *(Done: `dfc261e` — multi-threaded correctness tests added for all DLock2
  variants including DSM at 2/4/8 threads.)*

- [x] **Run baseline benchmarks on saturn (128T Intel Xeon Gold 6438M).**
  *(Done: 2026-02-26 — counter cs [1000,3000] noncs [0], all 16 lock variants,
  4-128 threads. Results in `visualization/output/`. FC-PQ achieves 0.9996 JFI
  at 128T with 0.74x FC throughput.)*

- [x] **Write experiment plan spec.**
  *(Done: 2026-02-26 — `docs/EXPERIMENT_PLAN.md`. 10 experiment groups covering
  CS ratio sweep, CS length crossover, non-CS sweep, response time, asymmetric
  contention, hash map, log buffer, queue/PQ, NUMA, perf profiling.)*

---

## Phase 1: Micro-Benchmark Experiment Suite

Full spec: [`docs/EXPERIMENT_PLAN.md`](docs/EXPERIMENT_PLAN.md)

These experiments use only the existing `counter-proportional` subcommand with
different configurations. No code changes needed.

Experiment scripts updated (2026-02-26): `experiment.nu` now includes CFL,
Ticket, CLH, PthreadMutex in all relevant groups. New functions added:
`group_benefits`, `group_tradeoff`, `group_combiner`, `group_perf`.

- [ ] **Group 1: CS ratio sweep.** *(EXPERIMENT_PLAN.md §Group 1)*
  CS ratios 1:1, 1:3, 1:10, 1:30, 1:100. All locks including CFL/Ticket/CLH/PthreadMutex, 4-128 threads.
  Validates that FC-PQ maintains JFI near 1.0 regardless of ratio.

- [ ] **Group 2: CS length scalability crossover.** *(§Group 2)*
  Uniform CS from 1 to 50000. FC, FC-PQ, FCBan, MCS, CFL, Mutex, SpinLock at 32T.
  Shows delegation advantage grows with CS length (L1 locality story).

- [ ] **Group 3: Non-CS sweep (contention levels).** *(§Group 3)*
  Non-CS: 0, 10, 100, 1K, 10K, 100K. FC, FC-PQ, FCBan, MCS, CFL, Mutex, USCL.
  Shows delegation advantage largest under high contention.

- [ ] **Group 4: Response time distributions.** *(§Group 4)*
  CS=1 and CS=1000,3000 with `--stat-response-time`. 8/16/32 threads.
  Includes CFL. Produces combiner vs waiter CDFs for the motivation figure.

- [ ] **Group 5: Asymmetric contention.** *(§Group 5)*
  Same CS, alternating non-CS (0 vs 10000). Includes CFL. Shows FC-PQ
  rebalances hold-time across hot/cold threads.

- [ ] **Group 8: Queue & priority queue.** *(§Group 8)*
  Enable existing queue/priority-queue benchmarks. LinkedList, VecDeque,
  BinaryHeap, BTreeSet. Shows delegation advantage on data structures.

- [ ] **Group Benefits: Delegation fairness Pareto scatter.** *(NEW)*
  CS ratio sweep at 32T (1:1 to 1:100), thread scaling at 1:10 ratio,
  counter-array data footprint. Produces the paper's central scatter plot
  showing FC-PQ uniquely in the high-throughput, high-fairness corner.
  Post-processing: `visualization/plot_pareto.py`.

- [ ] **Group 11: Delegation vs ShflLock head-to-head.** *(NEW)*
  3 dimensions: fairness under heterogeneous CS, L1 locality via data
  footprint scaling, tail latency under asymmetric contention.
  Scripted in `group_delegation_vs_shfl` in `experiment.nu`.

- [ ] **Group 12: Factor analysis (FC-PQ overhead decomposition).** *(NEW)*
  Decompose FC→FC-PQ throughput gap into PQ scheduling overhead.
  Inspired by TCLocks OSDI'23 Fig 5g. Compare FC, FC-Ban, FC-PQ at
  32T across CS ratios (uniform, 3:1, 1:10) and thread scaling.
  Scripted in `group_factor_analysis` in `experiment.nu`.

- [ ] **Bump duration to 30s for final paper runs.**
  TCLocks (OSDI'23) uses 30-second runs. Our 15s runs are for development
  iteration. For the camera-ready data, re-run with `--duration 30`.

---

## Phase 2: Combiner Response-Time Study

Experiment function: `group_combiner` in `experiment.nu`.
Post-processing: `visualization/analyze_combiner.py`.

- [ ] **Characterize combiner time distribution across thread counts.**
  Run FC, CC, FC-Ban, CC-Ban, FC-PQ, MCS, CFL at 4, 8, 16, 32, 64 threads.
  Measure combiner time fraction, produce box plots.
  Scripted in `group_combiner`. Analysis in `analyze_combiner.py`.

- [ ] **Implement tail-as-combiner selection for FC.** *(DEFERRED)*
  In `dlock2/fc/lock.rs`, modify combiner election so the tail of the thread
  list (last node executed) becomes the next combiner. Deferred — combiner
  penalty data without the fix is itself a contribution.

- [ ] **Measure combiner vs. waiter response time separately.**
  *(Infrastructure done: combiner/waiter split CDFs already generated by
  `report_response_times()` in `counter_common.rs`.)*
  For each lock variant, produce split CDFs. The key claim: unfair locks have
  bimodal response-time distributions; fair locks have more uniform ones.

- [ ] **Quantify combiner penalty scaling.**
  Plot: combiner_p99 / waiter_p50 vs. thread count. Show this ratio grows
  with concurrency for unfair variants, stays bounded for fair variants.
  Computed by `analyze_combiner.py --file-pattern "combiner-study-*"`.

---

## Phase 3: Fairness-Performance Tradeoff Validation (KEY EXPERIMENT)

This validates Contribution #3: delegation breaks the traditional
fairness-performance tradeoff because shared data stays in the combiner's
L1 regardless of serving order.

- [x] **Implement CFL baseline (required comparison).**
  *(Done: `ee262bf` — CFL-MCS implemented as `DLock2Wrapper<RawCflLock>`.
  Per-thread vLHT tracking with O(N) queue reordering during unlock.
  Smoke test: CFL JFI=0.992 at 4T but ~23% throughput loss vs MCS,
  while FC-PQ JFI=0.891 with only ~1.3% loss vs FC — validates the
  "delegation breaks the fairness-performance tradeoff" thesis.)*

- [x] **Implement MCS lock in DLock2 framework.**
  *(Done: `4d57e13` + `8e82f36` — MCS added as `DLock2Wrapper<RawMcsLock>`,
  uses per-lock ThreadLocal for queue nodes.)*

- [ ] **Run the tradeoff experiment.**
  Scripted in `group_tradeoff` in `experiment.nu`.
  - Workload: proportional counter with 3:1 CS ratio at 8/16/32/64 threads,
    plus wider ratios (1:10, 1:100) at 32T
  - Locks: FC, FC-PQ, FC-Ban, MCS, CFL, SpinLock, Mutex, Ticket, CLH
  - Measure: throughput AND JFI for each
  - Key plot: scatter plot of (throughput, JFI) showing Pareto frontier
  - Post-processing: `visualization/plot_pareto.py`
  - **Expected result:** FC→FC-PQ throughput gap is small (<10%?), while
    MCS→CFL throughput gap is large, because CFL pays cross-core migration
    on every fair handoff but FC-PQ doesn't.

- [ ] **Cache miss validation.**
  Scripted in `group_perf` in `experiment.nu`.
  Run `perf stat` on FC vs FC-PQ vs MCS vs CFL, collecting L1/LLC misses.
  FC and FC-PQ should have similar shared-data miss rates (data stays in
  combiner's L1). CFL should show higher shared-data misses due to
  cross-core migration on handoff.

---

## Phase 4: Additional Baselines

- [x] **Add Ticket Lock to Rust benchmarks.**
  *(Done: `df4b185` — RawTicketLock as RawMutex wrapped by DLock2Wrapper.
  Simple fetch-and-add + spin on now_serving. FIFO-fair but O(N)
  invalidations per handoff.)*

- [x] **Implement CLH lock in DLock2 framework.**
  *(Done: `87152f3` — RawClhLock as RawMutex wrapped by DLock2Wrapper.
  Spins on predecessor's node, implicit node recycling. FIFO-fair with
  O(1) remote memory references per handoff.)*

- [~] **~~Benchmark pthread_mutex with PTHREAD_MUTEX_ADAPTIVE_NP.~~**
  *(Removed: Rust's `std::sync::Mutex` already spins 100 iterations before
  blocking via futex, making it effectively adaptive. Redundant baseline.)*

---

## Phase 5: Optimizations from Related Work

Concrete code tasks inspired by CFL, ShflLock, Syncord, TCLocks
(see RESEARCH_PLAN.md Section 4.6).

- [x] **Newcomer usage initialization (from CFL).**
  *(Done: `8403d71` — combiner tracks `total_usage / total_served`, newcomers
  with usage=0 initialized to running average in both FCPQ and FCSL.)*

- [x] **Starvation counter (from ShflLock).**
  *(Done: `120d287` — per-node `pass_entered` counter in UsageNode, combined
  with a per-lock `combine_pass` counter. After 8 passes without service,
  usage clamped to queue minimum. <1% throughput impact.)*

- [x] **Prefetch next waiter's input (from TCLocks).**
  *(Done: `ae442fc` — `_mm_prefetch` with `_MM_HINT_T0` for next PQ root's
  data pointer before executing current delegate. Overlaps memory latency
  with CS execution.)*

- [ ] **(Optional) NUMA-aware tie-breaking (from CFL).**
  Among threads with similar usage (within epsilon), prefer the one on the
  same NUMA node as the combiner. Only relevant on multi-socket machines.
  Requires reading NUMA topology at init time.

- [ ] **(Optional) Hybrid admission control (from Syncord).**
  Add a "soft ban" to FC-PQ: if a thread's usage exceeds 2x the average,
  delay its re-insertion into the PQ by one combining pass. Combines strict
  enforcement of banning with work-conservation of priority scheduling.

---

## Phase 6: End-to-End Applications

Full spec: [`docs/EXPERIMENT_PLAN.md`](docs/EXPERIMENT_PLAN.md) §Group 6-7.

### A. Concurrent Hash Map *(§Group 6)*

- [ ] **Implement lock-protected hash map benchmark.**
  - `HashMap<u64, Vec<u8>>`, single global delegation lock
  - Operations: get (short CS ~100ns), put (medium CS ~500ns),
    scan (long CS ~5-50us iterating N entries)
  - Thread mix: N-2 lookup threads + 2 scan threads
  - Pre-populate 10K entries, Zipfian key distribution
  - New subcommand: `DLock2Experiment::HashMap`
  - New file: `src/benchmark/dlock2/hashmap.rs`

- [ ] **Measure per-operation-type latency.**
  Track response time separately for get, put, and scan operations. The key
  result: under unfair delegation, get latency degrades when scan threads
  monopolize; fair variants bound get tail latency.

- [ ] **Compare against per-bucket fine-grained locking.**
  Show that delegation (even fair) can outperform fine-grained locking on
  throughput while maintaining fairness — the best of both worlds.

### B. Producer-Consumer Log Buffer *(§Group 7)*

- [ ] **Implement log buffer benchmark.**
  - `VecDeque<LogEntry>`, single global delegation lock
  - N-1 producer threads: append single entry (short CS ~100ns)
  - 1 consumer thread: drain batch of K entries (long CS, K=100-500)
  - New subcommand: `DLock2Experiment::LogBuffer`
  - New file: `src/benchmark/dlock2/log_buffer.rs`

### C. (Optional) Concurrent Priority Queue / Task Scheduler

- [ ] **Implement mixed-operation priority queue benchmark.**
  - Operations: insert (variable batch size = variable CS), extract-min
    (short CS)
  - Thread groups: "producers" (bulk insert, long CS) vs "consumers"
    (extract-min, short CS)
  - Measures: consumer starvation under unfair vs. fair locks

---

## Phase 7: Multi-Machine & NUMA Evaluation

Full spec: [`docs/EXPERIMENT_PLAN.md`](docs/EXPERIMENT_PLAN.md) §Group 9.

- [x] **Primary testbed: saturn (Intel Xeon Gold 6438M, 2-socket, 128T).**
  *(Available. 2 NUMA nodes, Sapphire Rapids.)*

- [ ] **Obtain access to an AMD machine.**
  Need at least one AMD EPYC (Zen 3/4) for cross-vendor validation.
  Different L3 topology (CCX/CCD) and Infinity Fabric interconnect may
  change the delegation crossover point.

- [ ] **Run NUMA stress test on saturn.**
  Pin half threads to socket 0, half to socket 1 vs packed on one socket.
  Compare throughput ratio and `perf stat` cache misses across lock types.
  Delegation should show smaller NUMA penalty than MCS/Mutex.

- [ ] **Run full micro-benchmark suite on AMD machine.**
  Repeat Phase 1 experiments. Document topology with `lscpu`, `numactl
  --hardware`, and `lstopo`.

- [ ] **Consider H-Synch extension.**
  If NUMA effects are significant, implement NUMA-aware fair combining
  (per-NUMA-node queues with fair inter-node arbitration). This could be a
  contribution or future work depending on time.

---

## Phase 8: Overhead Analysis

Full spec: [`docs/EXPERIMENT_PLAN.md`](docs/EXPERIMENT_PLAN.md) §Group 10.

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

## Phase 9: Writing

- [ ] **Draft Section 3 (Design).**
  - Formal usage-fairness definition
  - Banning algorithm with pseudocode
  - Priority-based combining with pseudocode
  - Combiner selection strategy
  - Implementation notes (Rust, cache padding, memory ordering)

- [ ] **Draft Section 4 (Evaluation).**
  - Experimental setup (machines, methodology)
  - **Fairness-performance tradeoff figure** (the central result from Phase 3)
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
  - TCLocks: complementary (transparency vs fairness). TCLocks has working
    userspace port. Combining stack-switching with FC-PQ = future work.
    Frame: "TCLocks solves API transparency; we solve fairness."
  - QD Lock (Uppsala): automatic pthread-to-delegation transformation tool
  - ffwd: dedicated-server delegation, liblock LD_PRELOAD
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

## Phase 10: Stretch Goals (if time permits)

- [ ] **Async/coroutine combiner prototype.**
  Use Rust async/await to implement a combiner that can yield its
  non-critical section to a waiting thread. Even a proof-of-concept
  demonstrating the concept would significantly strengthen the paper's
  vision section.

- [ ] **Transparent fair delegation (TCLocks + FC-PQ).**
  Port TCLocks' userspace KOMB (~500 lines C in `rs3lab/TCLocks/src/userspace/litl/`)
  to Rust, integrating FC-PQ's priority scheduling into the combiner loop.
  Core work: `komb_context_switch()` via `core::arch::asm!`, thread-local
  shadow stacks, LD_PRELOAD shim as `cdylib`. ~530 lines Rust, medium-high
  risk (signal safety, Rust ownership vs raw stack manipulation). Would give
  transparent + fair delegation for unmodified applications. Key concern:
  extensive `unsafe` blocks for stack-switching fundamentally violates Rust
  ownership model.

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
| ~~JFI implementation~~ | ~~Phase 1, Phase 9~~ | Done (`a3131ac`) |
| ~~CFL baseline implementation~~ | ~~Phase 3~~ | Done (`ee262bf`) |
| ~~MCS baseline implementation~~ | ~~Phase 3~~ | Done (`4d57e13`) |
| Response time CDF tooling | Phase 2, Phase 6 | Reusable across all experiments |
| AMD machine access | Phase 7 | Cross-vendor validation |
| End-to-end app design | Phase 6, Phase 9 | Hash map + log buffer spec in EXPERIMENT_PLAN.md |
| Combiner penalty data | Phase 9 (intro figures) | The motivating figure needs this data |
| Scope the O(C_max) claim | Phase 9 | See note below |

---

## Priority Order (what to do next)

1. **Phase 1** — run micro-benchmark suite (zero code, high value)
   - Group Benefits + Group Tradeoff + Group 12 (factor analysis) first — they produce the central figures
   - Then Groups 1-5, 8, 11 for comprehensive data
2. **Phase 2** — combiner response-time study (group_combiner)
3. **Phase 3** — tradeoff validation + cache miss validation (group_perf)
4. **Phase 6** — end-to-end applications (hash map is critical for PPoPP/EuroSys)
5. **Phase 7** — multi-machine NUMA experiments
6. **Phase 8** — overhead analysis (perf stat)
7. **Phase 9** — writing

All Phase 0-5 code tasks (JFI, CDF tooling, CFL, MCS, baselines,
optimizations) are **done**. The immediate work is running experiments
and implementing the hash map application benchmark.

**Methodology note (2026-02-26):** After reading TCLocks (OSDI'23), we
adopted their factor analysis approach (Group 12), bumped final-run
duration to 30s, and added p99.99 as a latency metric. Their evaluation
does NOT measure fairness or heterogeneous CS — these are our unique
contributions.

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

2. **Which end-to-end apps?** Hash map + log buffer (spec in EXPERIMENT_PLAN.md).
   Hash map is the safest choice (well-understood, easy to argue relevance).
   Log buffer demonstrates write-dominated asymmetric workload.

3. **PPoPP vs. EuroSys?** If the tradeoff experiment + one end-to-end app
   are solid by Aug 2026, target PPoPP. If more time is needed, fall back
   to EuroSys (Oct 2026).

4. **Include async/coroutine prototype?** High reward but high risk. Decide
   after Phase 2 — if core evaluation is solid, it's better as future work.

5. **Rust-only or also C?** DLock2 (Rust) is the primary framework. C
   implementations exist but are less complete. Argue Rust's zero-cost
   abstraction makes it equivalent in performance. If reviewers push back,
   the C FC-Ban implementation can serve as validation.
