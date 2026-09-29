# Evidence index

This directory indexes the completed experiments that bear on the thesis: fairness by switching threads moves data, fairness by switching requests does not. Committed reports live on the branches listed in the table (`jj new <bookmark>` or `jj workspace add` to materialize). Raw `.worktree/` roots were git-ignored; on 2026-09-26 the roots cited below were moved off the removed worktrees to `~/Locks-artifacts/<branch-name>/<root>` (e.g. `~/Locks-artifacts/experiment-upscaledb-fc-pq-integration/upscaledb-joined-scaling/`), and every smoke, verification, build and superseded-campaign root was deleted. `.worktree/logp` in this checkout is a symlink into that archive. A pruned copy of the ledger is [`all-experiments-2026-09-25.md`](all-experiments-2026-09-25.md); the verbatim original stays on `experiment/upscaledb-fc-pq-integration@dcbfacb`. Results withdrawn on 2026-09-28 are listed under [Withdrawn](#withdrawn).

| Study | Branch@commit | Report path | Trials | Key result for the thesis |
|---|---|---|---:|---|
| Initial UpScaleDB integration | `experiment/upscaledb-fc-pq-integration@dcbfacb` | `plan/2026-09-23/upscaledb-integration-plan.md` (section "Fixed-work results") | 600 primary + 80 profile (whole study; only the cells here survive) | 4 workers on 4 CPUs; FC-PQ vs Native paired speedup 2.039x (packed), 1.129x (split). See finding 002. |
| Joined physical/NUMA/SMT scaling | `experiment/upscaledb-fc-pq-integration@dcbfacb` | `docs/reports/upscaledb-joined-dimensions/report.md`; raw `.worktree/upscaledb-joined-scaling/overview/scaling.csv` | 330 | P64 fixed: FC 4.07x, FC-PQ 2.93x, USCL 3.26x vs Native; CPU-s FC 72.74, FC-PQ 84.66, USCL 2.69, Native 12.00. P8 fixed: FC 1.73x, FC-PQ 1.51x, USCL 1.72x; CPU-s FC 8.43, FC-PQ 8.42, USCL 2.07, Native 9.23. See finding 003. |
| H1-H2 hypotheses | `experiment/upscaledb-fc-pq-integration@dcbfacb` | `docs/reports/upscaledb-hypotheses/report.md` | 192 | H2: U-SCL waits 2180/2131/1681/0.58 us at release->request 0/50/500/5000 us vs FC-PQ ~0.6 us (reservation = idle with backlog). |
| Boundary studies cost/arrival/database/tables | `experiment/upscaledb-fc-pq-integration@dcbfacb` | `docs/reports/upscaledb-boundaries/report.html`, `docs/reports/upscaledb-boundaries/provenance.json` | 264 (of 306) | USCL 8-table split 597829 -> 4617 op/s; arrival 95% read 1.1x load FC/FC-PQ ~909K/s vs USCL ~676K/s with ~460K backlog. |
| Other locks multitable | `experiment/multitable-other-locks@3f770d7` | `docs/reports/upscaledb-other-locks/report.html` | 180 | split/32 medians Mop/s: CLH 8.740, SpinLock 8.550, Ticket 8.060, MCS 7.151, FC 6.207, FC-PQ 5.704. |
| High contention hotspot | `experiment/multitable-other-locks@3f770d7` | `docs/reports/fairness-campaign/report.html`, `plan/2026-09-25/upscaledb-high-contention.md`; raw `.worktree/upscaledb-high-contention-validated/analysis-readable/report.html` | 450 + 27 diagnostic | 32W FC-PQ/MCS 0.743 uniform, 1.135 hot90, 1.105 hot100; FC-PQ absolute ~10.45M -> ~1.02M op/s. |
| Heterogeneous clients (single and read cells) | `experiment/heterogeneous-clients@d9f1abc` | driver `integration/upscaledb/heterogeneous_clients.py`; raw `.worktree/heterogeneous-trials-20260925-01/analysis/summary.txt` (no committed report) | 84 primary + 84 profile | 8 clients on CPUs 0-7. Shared single FC->FC-PQ profile Jain 0.896->0.977 (MCS 0.981, USCL 0.984); shared pure read FC-PQ/FC throughput 0.397x. |
| E0(b) FC-PQ fast-path ablation | `e0b/fcpq-fast-path@573b1f6` ([PR #48](https://github.com/taooceros/Locks/pull/48), open; default-off features) | `docs/evidence/e0b-fast-path-ablation/RESULTS.md`; raw `~/Locks-artifacts/e0b-fcpq-fast-path/raw/` | 3-10 per cell | `fcpq_fast_path`: 1W tax vs FC 64.7 ns -> -8.1 ns per request, 1W FC-PQ/FC 0.53 -> 1.12 (cs 1). Saturated 32W FC-PQ/FC still 0.65 (cs 1) / 0.88 (cs 1000); the fast path changes saturated per-op cost by only 2-5 ns. Microbenchmark (`counter-proportional`), not a DB study. |
| LogP model checks | `research/logp-analysis@555ad76` | `analysis/logp/README.md`; raw `.worktree/logp/verification.json` | 8 | Abstract fairness/performance separation; not hardware evidence. |

## Reading rules

- Primary (uninstrumented) and profile (service-Jain) runs are separate cohorts; do not combine them into one Pareto point.
- Fixed-work and duration modes are not interchangeable; duration changes the completed mix.
- 3-5 trial ranges are not confidence intervals.
- Latency percentiles are histogram bucket upper bounds.
- CPU-s is not energy; drain/join timing boundaries differ per study.

## Gaps relative to the thesis

- No cache-migration measurement (no HITM/LLC counters) in any study; locality claims are inference.
- CFL-local is an unverified proxy with cross-handle global accounting, not Park/Eom's CFL artifact.
- DLock2 waiters spin only, never park (see finding 002); every study keeps threads <= CPUs, and CPU-s numbers reflect the spin policy.
- Every FC-PQ result above except E0(b) predates `fcpq_fast_path` (PR #48, not merged). In E0(b) the fast path removes the 1-worker tax, but saturated FC-PQ still costs more per op than FC (32W FC-PQ/FC 0.65 / 0.88; P8 FC-PQ 13% below FC at identical CPU-s).

## Withdrawn

Removed on 2026-09-28 ([plan](../../plan/2026-09-28/evidence-prune.md)); the data is untouched at the locations given.

- **redb 1/64 external-lock transactions** (FC->FC-PQ Jain 0.891->0.992, MCS 0.907, small-txn 1.500x, total records 0.832x; ledger section 13): the harness wrapped whole transactions of unmodified redb, so redb's internal writer lock was never contended. `experiment/redb-fair-transactions@e96f5f9`; `~/Locks-artifacts/experiment-redb-fair-transactions/redb-campaign-20260925-04/`.
- **Initial UpScaleDB, oversubscribed fixed-work cells** (packed 8W 0.494x and 16W 0.209x, split 16W 0.157x on 4 CPUs; packed-8W stage decomposition; the README "4-CPU/8-worker collapse" row): spin-only waiters with threads > CPUs. `experiment/upscaledb-fc-pq-integration@dcbfacb:plan/2026-09-23/upscaledb-integration-plan.md`; `~/Locks-artifacts/experiment-upscaledb-fc-pq-integration/upscaledb/`.
- **Initial UpScaleDB, 120 s sustained mode** (4 finders + 4 inserters on 4 CPUs, 480 CPU-s, profile Jain 0.824/0.852/0.902; ledger section 5.2) and the packed-8W profile perturbation (+32.4%/+20.7%): 8 threads on 4 CPUs. Same branch path and archive dir as above.
- **1-worker "constant tax" claims** (FC-PQ +28% vs Native at 1W, i.e. 0.777x; "13-28% over FC at 1 worker"): measured before `fcpq_fast_path`; superseded by E0(b) (PR #48). Same branch path and archive dir as above.
- **Initial UpScaleDB, packed 2W** (FC-PQ vs Native 0.909x; ledger section 5.1): low-occupancy, pre-fast-path result of the same kind as the 1-worker tax. Same branch path and archive dir as above.
- **Boundary cost study, one_equal cells** (FC-PQ/FC 0.665-0.809, CPU/op 1.24-1.51x; ledger section 9.1): single-worker, pre-fast-path. `experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/upscaledb-boundaries/report.html`; `~/Locks-artifacts/experiment-upscaledb-fc-pq-integration/upscaledb-boundaries/` (`cost/`).
- **Boundary database study, single-worker cells** (FC-PQ/FC 0.835-0.838 small, 0.888-0.896 large read domain; ledger section 9.3): single-worker, pre-fast-path. Same branch path as above; same archive dir (`database/`).
- **Heterogeneous clients, batch8 cells** (shared Jain 0.579->0.628, reader share 7.3%->11.5%, USCL 0.908; split batch8): batch8 is application restructuring, a README non-goal. `experiment/heterogeneous-clients@d9f1abc`; `~/Locks-artifacts/experiment-heterogeneous-clients/heterogeneous-trials-20260925-01/` (`batch-*`).
- **Historical microbenchmarks** (saturn counter, 16 locks, 4-128T; 1 s single-trial demo tables; ledger section 3): the ledger itself calls them non-evidence and their raw data is lost. `experiment/upscaledb-fc-pq@7aa071d:docs/EXPERIMENT_RESULTS_DEMO.md` and `TODO.md`; copy in [`docs/archive/`](../archive/README.md).
- **H3 artificial burst/sleep** (5 ms sleep every 64 ops; ledger section 8): the ledger itself calls it a sensitivity appendix, not realistic evidence. `experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/upscaledb-hypotheses/report.md`; `~/Locks-artifacts/experiment-upscaledb-fc-pq-integration/upscaledb-hypotheses/`.
- **Admission accounting A/B** (200 trials, counted vs external admission; ledger section 6): it compared two variants of the concurrent stop/drain bridge lifecycle, which was then removed in favour of caller join + exclusive destroy, and it ran pre-fast-path FC-PQ; its result was null and the lifecycle decision did not rest on it. `experiment/upscaledb-fc-pq-integration@dcbfacb:plan/2026-09-24/upscaledb-admission-accounting.md`; `~/Locks-artifacts/experiment-upscaledb-fc-pq-integration/upscaledb-admission-study/`.
- **CFL smoke test** (CFL JFI 0.992 at 4T with ~23% loss vs MCS, FC-PQ JFI 0.891 with ~1.3% loss vs FC; old `TODO.md` note): one smoke run of the unverified `cfl` Rust port, presented as validating the thesis. Commit `ee262bf` (in `main` history); text kept in [`docs/archive/TODO-pre-thesis-2026-09-26.md`](../archive/TODO-pre-thesis-2026-09-26.md); no raw data archived.
