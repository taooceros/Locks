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

- [x] **Implement MCS lock in DLock2 framework.**
  *(Done: `4d57e13` + `8e82f36` — MCS added as `DLock2Wrapper<RawMcsLock>`,
  uses per-lock ThreadLocal for queue nodes.)*
- [x] **Fix FC-SL lost-request hang + usage data races** (2026-09-28, [plan](plan/2026-09-28/fcsl-lost-request-fix.md)): 2/12 hangs before, 350/350 passes after.

- [x] **Prune withdrawn evidence (2026-09-28).** [Plan](plan/2026-09-28/evidence-prune.md);
  removed redb external-lock, oversubscribed, stale 1-worker-tax and other low-occupancy
  pre-fast-path, batch8, historical-demo, H3 burst/sleep, admission-accounting, 120 s and
  CFL smoke-test results; provenance in [Withdrawn](docs/evidence/README.md#withdrawn).

---

## E0 Prerequisites (engineering, not results)

- [ ] **(a) Shelved, no longer a prerequisite: spin-then-park waiters in FC and FC-PQ.**
  Parking is deferred; waiters stay spin-only and every experiment keeps
  threads <= CPUs. Unmerged: spin-then-park change `uwqronmm` (`spin_park`
  feature) and the E0(a') timed-park comparison `zlqxmnyz`.

- [~] **(b) Trim FC-PQ per-request tax.**
  Sample `__rdtscp` every k requests; bound heap arity.
  Target: FC-PQ/FC >= 0.95 at 1 worker. Met by `fcpq_fast_path` in
  [PR #48](https://github.com/taooceros/Locks/pull/48) (1W FC-PQ/FC 1.12);
  features are default-off and the default is still to be decided.

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

## E3 Database confirmation (spin-only, threads <= CPUs)

- [ ] **UpScaleDB single-operation integration.** Preload size (1K vs 1M
  records) as the W knob, plus perf counters.
- [ ] **redb 1/64 write-transaction mix** as the application endpoint,
  regenerated with the internal-lock harness ([guide](integration/redb/README.md)).
- [ ] **Run both via [`integration/README.md`](integration/README.md) workflows.**

---

## E4 SCL-fidelity (only if E1-E3 hold)

- [ ] **Disk-backed UpScaleDB with fsync.** 4 find + 4 insert on 8 CPUs
  (threads <= CPUs), 120 s, lock-opportunity accounting. The original SCL
  setup (4 + 4 on 4 CPUs) oversubscribed.

---

## Success criteria

- [ ] **E1.** Crossover W\* exists on both same-socket and cross-socket with
  ranges not overlapping over >= 5 trials; FC-PQ Jain >= 0.95 at all W;
  U-SCL idle-with-backlog > 0 where switching locks idle 0.
- [ ] **E3.** Ordering from E1 reproduced in UpScaleDB and redb.
- [ ] **Kill criterion.** If real CFL is flat in W on cross-socket, the thesis
  reduces to work-conservation vs U-SCL only; record and reframe.
