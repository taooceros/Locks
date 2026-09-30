# TODO: thesis checklist

Research plan: [README.md](README.md). Previous roadmap:
[docs/archive/TODO-pre-thesis-2026-09-26.md](docs/archive/TODO-pre-thesis-2026-09-26.md).

Status legend: `[ ]` not started, `[~]` in progress, `[x]` done

---

## Done (infrastructure)

- [x] **Theoretical analysis: separate performance and fairness models.**
  *(Done on `research/logp-analysis`, merged 2026-09-26: [analysis artifact](analysis/logp/README.md),
  [story draft](analysis/logp/story-draft.tex), eight passing deterministic
  checks. Establishes the service-sequence/executor-sequence framework, the
  conditional crossover b(M+h-d) > M+A+K, and the limits of current FC-PQ's
  fairness bound. All ten evidence slots remain open; they map to E1-E3 below.)*

- [x] **Reorganize local artifacts for git worktree use.**
  *(Done: 2026-03-28 — benchmark runs default to `.worktree/output`, perf
  profiling writes to `.worktree/profiles`, analysis scripts resolve output
  paths dynamically; fallback support remains for older checkouts.)*

- [x] **Integrate database experiments without changing application logic.**
  [Guide](integration/README.md) and
  [verification record](plan/2026-09-25/experiment-setup-integration.md).
  Corrected the assistant's overbroad removal: restored single-operation UpScaleDB
  integration and Native/mutex controls, without batch8 or application restructuring.
  Verified 22 fresh binaries, 69 DB gates, 22 exact-content fixed-work smokes,
  21 Python tests, 8+11 bridge tests and the workspace build.
  Kept redb, shared lock fixes, original worktrees and historical evidence.

- [x] **Group integration tools and clarify the runner workflow.**
  Removed the live dashboard; separated core, runner, specialized experiments,
  reports and tests with folder READMEs; colocated redb's controller and workload.
  Updated module imports, source/compiler paths, CI and the build/check/run/analyze guide.
  Path-cutover checks: 21 Python tests, 18 CLI entrypoints, five companion include
  checks, a fresh FC-PQ build/run/analysis and 20 redb smoke cells passed.
  No formal matrices or documentation-only verification reruns.

- [x] **Extract repeated runner mechanics and improve filenames.**
  Four controllers share captured/logged process execution without a runner framework;
  study/report names describe their purpose. Preserved timeout policies and artifacts.
  Verified 28 Python tests, 14 renamed CLI entrypoints, shared capture of a real DB
  error gate, and shared logged execution through a real trial and renamed analyzer.

- [x] **Integrate delegation locks inside redb's write path (UpScaleDB-style).**
  [Guide](integration/redb/README.md). Replaced the external-wrapper harness over
  unmodified redb with pinned redb 3.1.0 + numbered patches: one fixed-shape write
  body shared by `refactored` (original Mutex/Condvar) and the bridge
  (`bridge_mutex`/`mcs`/`fc`/`fc_pq`); `native` stays upstream. Admission-only
  replacement deliberately not built (decision in the guide). Verified: 54/54 DB
  gate cases, 165 upstream redb tests on the patched tree, a broken-lock negative
  control tripping the writer assertion, and 72 fresh-process smoke cells with
  provenance (refactored vs native within repeat noise). No formal matrix run.

- [x] **Implement CFL baseline (required comparison).**
  *(Done: `ee262bf` — CFL-MCS implemented as `DLock2Wrapper<RawCflLock>`.
  Per-thread vLHT tracking with O(N) queue reordering during unlock.
  Smoke test: CFL JFI=0.992 at 4T but ~23% throughput loss vs MCS,
  while FC-PQ JFI=0.891 with only ~1.3% loss vs FC — validates the
  "delegation breaks the fairness-performance tradeoff" thesis.)*

- [x] **Implement MCS lock in DLock2 framework.**
  *(Done: `4d57e13` + `8e82f36` — MCS added as `DLock2Wrapper<RawMcsLock>`,
  uses per-lock ThreadLocal for queue nodes.)*
- [x] **Fix FC-SL lost-request hang + usage data races** (2026-09-28, [plan](plan/2026-09-28/fcsl-lost-request-fix.md)): 2/12 hangs before, 350/350 passes after.

---

## E0 Prerequisites (engineering, not results)

- [x] **(a) Spin-then-park waiters in FC and FC-PQ.**
  `crates/libdlock/src/dlock2/fc/lock.rs` and
  `crates/libdlock/src/dlock2/fc_pq/lock.rs`; reuse the
  `crates/libdlock/src/parker/block_parker.rs` design; feature-flagged.
  *(Done: 2026-09-27 on `e0-engineering`, [plan](plan/2026-09-27/e0a-spin-then-park.md).
  `spin_park` cargo feature, default off; per-node futex in `dlock2/park.rs`,
  100 us spin budget (`DLOCK_SPIN_BEFORE_PARK_US` at build time), combiner
  wakes after publishing each result, `release_combiner()` re-checks parked
  waiters after every unlock, SeqCst enroll/retire handshake so a parked
  waiter is always enrolled; protocol in `dlock2/README.md`. Tests pass in
  both modes incl. new `idle_holder_release`. Smoke (cs=1000, 8 threads on
  4 CPUs): FC-PQ +4-10% throughput, FC within noise, CPU-s 16.0 -> 15.4-15.9;
  cs=20000: FC +19%, FC-PQ +10%, CPU-s 16.0 -> 13.5-15.1. 8 threads on 8
  CPUs: -1 to -4%. Tuning of the budget is left to E1/E3.)*

- [x] **(a') Wake-on-pick blocking waiters (`block_park`).**
  *(Implemented 2026-09-30 in `fcpq-block`,
  [plan](plan/2026-09-30/fcpq-block-wake-on-pick.md). TCLocks-style
  `WAITING -> PARKED` (waiter CAS) / `* -> PICKED` (combiner swap, wake iff
  parked) on the E0(a) park word; wake at pick (FC scan, FC-PQ pop) before
  the delegate, optional FC-PQ lookahead `DLOCK_WAKE_LOOKAHEAD=1`; no
  deferred fence. Handshakes 2 and 3 kept. Loom model
  `dlock2/park/loom_model.rs`; `thread_churn` test ported for FC and
  FC-PQ. Measurement by the fcpq-block bench/redb runs.)*

- [ ] **(b) Trim FC-PQ per-request tax.**
  Sample `__rdtscp` every k requests; bound heap arity.
  Target: FC-PQ/FC >= 0.95 at 1 worker.

- [ ] **(c) Obtain and validate the real CFL (Park/Eom, PPoPP'24).**
  Replace `cfl_local` as the CFL comparison.

---

## E1 Working-set sweep (core figure)

- [ ] **Workload.** Synthetic critical section touching W cache lines,
  W in {1, 4, 16, 64, 256, 1024, 4096}, fixed compute per CS.
- [ ] **Threads.** 8/16/32, one per physical core; request cost heterogeneity
  1:8 on half the threads.
- [ ] **Locks.** MCS, ticket, CLH, CFL, U-SCL, FC, FC-PQ.
- [ ] **Metrics.** Throughput, service Jain, idle-with-backlog time, per-op
  L2/LLC misses and HITM via `perf stat`.
- [ ] **Prediction check.** Switching locks fall with W, FC/FC-PQ flat, U-SCL
  Jain high but idle grows; headline = crossover W\*.
- [ ] **Placement.** Same-socket and cross-socket (NUMA) so D_migrate is
  non-trivial.
- [ ] **Harness.** `src/benchmark` counter-array style workload (existing
  `counter-proportional` with data footprint) as an `experiment.nu` group.

---

## E2 Fairness-granularity sweep

- [ ] **Vary CFL/U-SCL slice length and the FC-PQ selection window.**
- [ ] **Plot throughput vs achieved max usage gap.**
- [ ] **Check FC-PQ sits off the switching locks' trade-off curve.**

---

## E3 Database confirmation (after E0(a))

- [ ] **UpScaleDB single-operation integration.** Preload size (1K vs 1M
  records) as the W knob, plus perf counters.
- [ ] **redb 1/64 write-transaction mix** as the application endpoint.
- [ ] **Run both via [`integration/README.md`](integration/README.md) workflows.**

---

## E4 SCL-fidelity (only if E1-E3 hold)

- [ ] **Disk-backed UpScaleDB with fsync.** 4 find + 4 insert on 4 CPUs,
  120 s, lock-opportunity accounting.

---

## Success criteria

- [ ] **E1.** Crossover W\* exists on both same-socket and cross-socket with
  ranges not overlapping over >= 5 trials; FC-PQ Jain >= 0.95 at all W;
  U-SCL idle-with-backlog > 0 where switching locks idle 0.
- [ ] **E3.** Ordering from E1 reproduced in UpScaleDB and redb.
- [ ] **Kill criterion.** If real CFL is flat in W on cross-socket, the thesis
  reduces to work-conservation vs U-SCL only; record and reframe.
