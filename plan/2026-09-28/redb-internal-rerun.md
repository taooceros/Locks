# redb inside the write path: service-time fairness, client sweep, U-SCL, FC-PQ fast path

Status: Approved 2026-09-28 (user task assignment; counts as plan approval).
Implemented in workspace `redb-internal`; smoke, cache-counter cohort and
formal matrix run (see Formal, Cache counters, Reading). Scope: `integration/redb/**` only (no libdlock or
`crates/upscaledb-bridge` change was needed). Extends the internal-lock
harness of
[experiment-setup-integration](../2026-09-25/experiment-setup-integration.md).
The older external-lock redb results are invalid (redb's own writer lock was
never contended; `integration/redb/README.md`). This harness's formal matrix
replaces them, subject to the frequency caveat in the Formal section. The
caveat is quantified by the clock-normalised perf cohort
[redb-perf-02-clock](./redb-perf-02-clock.md) (section "Follow-up 2" below).

## Goal

Make the redb experiment answer the thesis question inside a real write path:
does FC-PQ give every client an equal share of the serialised write path
(service time), at what throughput relative to FC, MCS, U-SCL and native redb?

## Changes (all five requested)

1. **Service-time fairness.** Patched redb's shared body (patch 0002) reads the
   TSC with `rdtscp` once `begin` has returned and again once commit/abort has
   returned, on the executing thread (combiner for FC/FC-PQ, requester for the
   others), and returns the ticks with the outcome (`Served`). The requester is
   charged. `native` measures the same span around the same public calls in the
   harness. The analysis reports `service_jain`, each client's service share,
   `long_service_share`, `service_utilization` next to `tx_jain`.
   - Decision: the span starts **after** `begin`, identically for all variants,
     because the tracker's admission wait happens inside `begin_write`; this
     excludes `WriteTransaction` construction from service for every variant.
   - Decision: **always on**, no profile build. Overhead on one smoke cell
     (below): ≈1.2 % tx/s, lower in 8/8 pairs, inside the smoke's repeat spread.
     `build.py --uninstrumented` exists only for this check; run.py and the gate
     refuse it.
2. **`None` is the primary regime, `Immediate` a control.** Runner order,
   analysis table, plot rows and README text list `None` first.
3. **Client sweep {1, 2, 4, 8}.** A c-client cell pins c requesters to the first
   c prepared CPUs and restricts the trial process to them (`taskset`); never
   more threads than CPUs. Cohorts: `all1` = c × 1 record; `half1_halfK` = half
   the requests 1 record, half K: c/2 + c/2 clients for even c (2 clients =
   one 1-record + one K-record client), one client alternating 1, K at c = 1. The 8-worker hardcoding (main.rs
   `WORKERS`, run.py `len(workers) == 8`, "eight distinct CPUs") is gone.
4. **`uscl` variant.** U-SCL via the redb bridge, on the requester, using the
   `crates/upscaledb-bridge` pattern (boxed fairlock, bridge-mode init, weight
   1024, destroy after every submitter is `pthread_join`ed; `with_deadline` in
   the self-tests now joins explicitly for this reason). The gate runs every
   case for `uscl` and asserts its bodies never run on another thread.
5. **FC-PQ with `fcpq_fast_path`.** Harness features `fcpq_fast_path` and
   `fcpq_fast_path_stat` forward to libdlock; `build.json` records requested,
   Cargo-resolved (`cargo metadata`) and binary-reported (`--build-info`)
   features per binary, and prepare refuses a build without the fast path. The
   hit rate per cell is reported (the counter is E0(b)'s +0.2 ns diagnostic, so
   it stays in the timed build).

Files: `integration/redb/{Cargo.toml, build.py, correctness.py, run.py,
README.md, patches/0002-fixed-insert-write-body.patch, src/main.rs,
src/writer.rs, src/selftest.rs}`.

## Risks and caveats recorded before the run

- Waiting differs by lock: MCS, FC and FC-PQ spin; native, refactored and
  bridge_mutex block (redb's Mutex/Condvar, `std::sync::Mutex`); U-SCL may
  `sched_yield`, futex-wait or `nanosleep` (its ban). No cell oversubscribes.
- Service is TSC wall time inside the body, not CPU time.
- The instrumentation is in every timed binary; its measured cost is below.

## Verification

All on 2026-09-28, host 2 × Xeon Gold 6438M (128 logical CPUs), kernel 6.17.7.

- **Build**: `python3 -m integration.redb.build --output-dir .worktree/redb-build-02`
  (shared `MEASUREMENT_LOCK`): 3 binaries, no harness warnings; resolved
  features: patched/test_hooks → libdlock `fcpq_fast_path`,
  `fcpq_fast_path_stat`, redb `dlock_service_time`; native → `service_time`.
- **Correctness gate** (exclusive lock): `correctness.py --build-dir
  .worktree/redb-build-02` → **64/64 cases passed** (7 variants; 54 before plus
  10 for `uscl`). Evidence: `uscl` bodies on another thread = 0 in both
  test-hook stress cases; FC 3000/3200 (Immediate) and 9007/9600 (None), FC-PQ
  3111/3200 and 9424/9600; lone FC-PQ requester 90/90 fast-path hits; charged
  service / wall = 0.957-0.994 in every stress case, and test-hook conservation
  (charged == executed) held.
- **Upstream redb tests** on a copy of the patched tree (`cargo test --release
  --lib --test basic_tests --test integration_tests --test multithreading_tests`):
  37 + 69 + 56 + 3 = **165 passed**, 0 failed. The patched tree hash
  (`94132b5f…`) is the one both builds used.
- **Instrumentation overhead** (throwaway `.worktree/overhead/overhead.py`,
  exclusive lock): `fc_pq`, `all1`, None, 8 clients, CPUs 16-23, 8 ABAB pairs
  of 2 s runs of `redb-build-02` vs `redb-build-noinst-02` (same features minus
  `service_time`, `fcpq_fast_path_stat`). Instrumented 33,464 tx/s [29,836,
  34,091], uninstrumented 33,691 [33,377, 34,308]; paired ratio median 0.988
  [0.894, 0.995], instrumented lower in 8/8 pairs. Two `rdtscp` per ≈30 µs
  request predict ≈0.1 %; the remainder is probably code layout [INFERENCE].
- **Smoke**: 336 cells, **0 failed**, 06:56:40-07:08:44 (2.16 s per cell).


## Smoke acceptance and control results

**Smoke only: 3 repetitions per cell, 2 s windows. This is a setup and control
check, not a formal result**; ranges are wide and several cells are bimodal.

Setup: run root `.worktree/redb-smoke-01` (ignored), binaries from
`.worktree/redb-build-02`, CPUs 16-23 (8 distinct physical cores, socket 0,
node 0), `numactl --membind=0`, ext4. `uptime` before: load 3.48 (idle cores
checked: no CPU above 6 % busy over 5 s). Commands exactly as in
`integration/redb/README.md` (prepare, `--smoke`, `--analyze-smoke`). Output
`analysis-smoke/{summary.json,summary.md,throughput.png}`.

### Smoke table

Median [min, max] over the 3 repetitions. tx/s and records/s count requests
completed inside the 2 s window. `long svc share` = service share of the
64-record half (0.5 = equal service). CPU-s = process CPU seconds over the run.
Fast-path hit rate = FC-PQ fast-path hits / requests.

| durability | cohort | clients | variant | tx/s | records/s | service_jain | tx_jain | long svc share | CPU-s | fast-path hit rate |
|---|---|---|---|---|---|---|---|---|---|---|
| none | all1 | 1 | native | 35880 [33652, 36103] | 35880 [33652, 36103] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | refactored | 36739 [36521, 37100] | 36739 [36521, 37100] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | bridge_mutex | 37178 [37152, 37784] | 37178 [37152, 37784] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | mcs | 37014 [36774, 37660] | 37014 [36774, 37660] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | uscl | 37379 [36766, 37506] | 37379 [36766, 37506] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | fc | 37162 [36940, 37482] | 37162 [36940, 37482] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | fc_pq | 37246 [37158, 37432] | 37246 [37158, 37432] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | 1.000 [1.000, 1.000] |
| none | all1 | 2 | native | 33999 [33632, 34310] | 33999 [33632, 34310] | 0.704 [0.594, 0.927] | 0.697 [0.609, 0.918] | — | 2.17 [2.16, 2.17] | — |
| none | all1 | 2 | refactored | 34866 [34114, 35234] | 34866 [34114, 35234] | 0.555 [0.500, 0.674] | 0.552 [0.500, 0.689] | — | 2.17 [2.16, 2.17] | — |
| none | all1 | 2 | bridge_mutex | 34812 [32332, 34872] | 34812 [32332, 34872] | 0.951 [0.915, 0.992] | 0.946 [0.907, 0.999] | — | 2.25 [2.24, 2.25] | — |
| none | all1 | 2 | mcs | 29444 [27692, 29614] | 29444 [27692, 29614] | 1.000 [0.997, 1.000] | 1.000 [1.000, 1.000] | — | 4.00 [4.00, 4.00] | — |
| none | all1 | 2 | uscl | 35304 [34674, 35474] | 35304 [34674, 35474] | 1.000 [1.000, 1.000] | 1.000 [0.999, 1.000] | — | 3.99 [3.99, 4.00] | — |
| none | all1 | 2 | fc | 35625 [35387, 35670] | 35625 [35387, 35670] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 4.00 [4.00, 4.00] | — |
| none | all1 | 2 | fc_pq | 35469 [35428, 35568] | 35469 [35428, 35568] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 4.00 [4.00, 4.00] | 0.177 [0.171, 0.190] |
| none | all1 | 4 | native | 12230 [8707, 13867] | 12230 [8707, 13867] | 0.774 [0.749, 0.974] | 0.566 [0.497, 0.900] | — | 2.24 [2.23, 2.31] | — |
| none | all1 | 4 | refactored | 16793 [16577, 32612] | 16793 [16577, 32612] | 0.416 [0.363, 0.456] | 0.324 [0.311, 0.345] | — | 2.22 [2.22, 2.23] | — |
| none | all1 | 4 | bridge_mutex | 11010 [8192, 34711] | 11010 [8192, 34711] | 0.937 [0.323, 0.943] | 0.715 [0.331, 0.842] | — | 2.34 [2.33, 2.44] | — |
| none | all1 | 4 | mcs | 15345 [15174, 26403] | 15345 [15174, 26403] | 1.000 [0.998, 1.000] | 1.000 [1.000, 1.000] | — | 8.00 [8.00, 8.00] | — |
| none | all1 | 4 | uscl | 9352 [9109, 10286] | 9352 [9109, 10286] | 1.000 [1.000, 1.000] | 0.992 [0.989, 0.998] | — | 3.93 [3.93, 3.96] | — |
| none | all1 | 4 | fc | 20562 [20534, 31272] | 20562 [20534, 31272] | 0.998 [0.998, 0.999] | 1.000 [1.000, 1.000] | — | 8.00 [8.00, 8.00] | — |
| none | all1 | 4 | fc_pq | 20818 [20600, 31776] | 20818 [20600, 31776] | 0.999 [0.995, 0.999] | 0.996 [0.991, 0.997] | — | 8.00 [8.00, 8.00] | 0.000 [0.000, 0.000] |
| none | all1 | 8 | native | 8812 [8517, 21310] | 8812 [8517, 21310] | 0.925 [0.133, 0.946] | 0.760 [0.134, 0.869] | — | 2.33 [2.17, 2.35] | — |
| none | all1 | 8 | refactored | 10228 [8210, 32838] | 10228 [8210, 32838] | 0.820 [0.125, 0.972] | 0.619 [0.125, 0.906] | — | 2.30 [2.25, 2.35] | — |
| none | all1 | 8 | bridge_mutex | 7708 [7656, 8242] | 7708 [7656, 8242] | 0.983 [0.910, 0.988] | 0.873 [0.711, 0.930] | — | 2.37 [2.35, 2.37] | — |
| none | all1 | 8 | mcs | 14912 [14893, 15000] | 14912 [14893, 15000] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 16.00 [16.00, 16.00] | — |
| none | all1 | 8 | uscl | 8384 [8061, 8744] | 8384 [8061, 8744] | 1.000 [1.000, 1.000] | 0.999 [0.996, 1.000] | — | 3.93 [3.92, 3.93] | — |
| none | all1 | 8 | fc | 20360 [20096, 31545] | 20360 [20096, 31545] | 0.996 [0.995, 0.998] | 1.000 [1.000, 1.000] | — | 16.00 [16.00, 16.00] | — |
| none | all1 | 8 | fc_pq | 21351 [21302, 32074] | 21351 [21302, 32074] | 0.999 [0.998, 0.999] | 0.998 [0.996, 0.999] | — | 16.00 [16.00, 16.00] | 0.000 [0.000, 0.000] |
| none | half1_half64 | 1 | native | 17801 [17772, 17832] | 578532 [577558, 579508] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | refactored | 18932 [18796, 18940] | 615290 [610838, 615550] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | bridge_mutex | 19078 [18988, 19182] | 620003 [617078, 623416] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | mcs | 19094 [19072, 19166] | 620523 [619808, 622863] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | uscl | 18976 [18924, 19138] | 616720 [615030, 621986] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | fc | 18934 [18892, 19030] | 615356 [613958, 618476] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | fc_pq | 18768 [18418, 19050] | 609960 [598553, 619093] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | 1.000 [1.000, 1.000] |
| none | half1_half64 | 2 | native | 11494 [11470, 19326] | 734112 [256018, 735616] | 0.500 [0.500, 0.994] | 0.500 [0.500, 0.728] | 1.000 [0.540, 1.000] | 2.07 [2.06, 2.08] | — |
| none | half1_half64 | 2 | refactored | 12564 [12431, 35250] | 795584 [35250, 804064] | 0.500 [0.500, 0.500] | 0.500 [0.500, 0.500] | 1.000 [0.000, 1.000] | 2.08 [2.06, 2.09] | — |
| none | half1_half64 | 2 | bridge_mutex | 10126 [8071, 17280] | 516544 [477778, 648064] | 0.500 [0.500, 0.904] | 0.500 [0.500, 0.977] | 1.000 [0.663, 1.000] | 2.13 [1.73, 2.15] | — |
| none | half1_half64 | 2 | mcs | 16710 [16512, 16877] | 543043 [536640, 548502] | 0.873 [0.872, 0.877] | 1.000 [1.000, 1.000] | 0.691 [0.687, 0.691] | 4.00 [4.00, 4.00] | — |
| none | half1_half64 | 2 | uscl | 22748 [19200, 23036] | 417915 [347556, 424472] | 1.000 [1.000, 1.000] | 0.833 [0.827, 0.834] | 0.500 [0.500, 0.510] | 3.99 [3.25, 3.99] | — |
| none | half1_half64 | 2 | fc | 18473 [18206, 18646] | 599711 [591600, 605302] | 0.846 [0.843, 0.849] | 1.000 [1.000, 1.000] | 0.713 [0.711, 0.716] | 4.00 [4.00, 4.00] | — |
| none | half1_half64 | 2 | fc_pq | 20338 [19799, 20550] | 506438 [456988, 510856] | 0.961 [0.955, 0.969] | 0.941 [0.918, 0.948] | 0.601 [0.589, 0.608] | 4.00 [4.00, 4.00] | 0.244 [0.228, 0.294] |
| none | half1_half64 | 4 | native | 11154 [10968, 17850] | 701920 [573352, 713888] | 0.250 [0.250, 0.398] | 0.250 [0.250, 0.500] | 1.000 [0.753, 1.000] | 2.08 [2.07, 2.11] | — |
| none | half1_half64 | 4 | refactored | 9360 [8220, 10876] | 459521 [199089, 599040] | 0.288 [0.250, 0.831] | 0.322 [0.250, 0.491] | 0.928 [0.602, 1.000] | 2.08 [2.06, 2.17] | — |
| none | half1_half64 | 4 | bridge_mutex | 12452 [12044, 14056] | 770848 [739154, 796896] | 0.250 [0.250, 0.283] | 0.250 [0.250, 0.355] | 1.000 [0.937, 1.000] | 2.22 [2.19, 2.22] | — |
| none | half1_half64 | 4 | mcs | 12290 [8649, 14060] | 399457 [281061, 456950] | 0.821 [0.819, 0.890] | 1.000 [1.000, 1.000] | 0.699 [0.676, 0.727] | 8.00 [8.00, 8.00] | — |
| none | half1_half64 | 4 | uscl | 5784 [5648, 5996] | 108191 [104810, 109128] | 1.000 [1.000, 1.000] | 0.836 [0.828, 0.838] | 0.504 [0.504, 0.505] | 3.96 [3.94, 3.99] | — |
| none | half1_half64 | 4 | fc | 11263 [10419, 16404] | 359716 [343185, 539115] | 0.832 [0.828, 0.844] | 0.999 [0.999, 0.999] | 0.724 [0.715, 0.726] | 8.00 [8.00, 8.00] | — |
| none | half1_half64 | 4 | fc_pq | 14838 [14469, 17154] | 230370 [227242, 288653] | 0.995 [0.989, 0.997] | 0.783 [0.771, 0.802] | 0.465 [0.448, 0.474] | 8.00 [8.00, 8.00] | 0.000 [0.000, 0.000] |
| none | half1_half64 | 8 | native | 10710 [9640, 32203] | 599320 [32203, 685440] | 0.125 [0.125, 0.134] | 0.125 [0.125, 0.132] | 0.966 [0.000, 1.000] | 2.10 [1.80, 2.24] | — |
| none | half1_half64 | 8 | refactored | 9846 [6000, 17696] | 502764 [231634, 630176] | 0.236 [0.234, 0.629] | 0.246 [0.217, 0.622] | 0.847 [0.631, 1.000] | 2.15 [2.10, 2.24] | — |
| none | half1_half64 | 8 | bridge_mutex | 10640 [4091, 11536] | 680928 [221788, 738272] | 0.125 [0.125, 0.545] | 0.125 [0.125, 0.649] | 1.000 [0.931, 1.000] | 2.23 [2.06, 2.26] | — |
| none | half1_half64 | 8 | mcs | 9970 [8668, 12988] | 324024 [281742, 422110] | 0.862 [0.815, 0.888] | 1.000 [1.000, 1.000] | 0.700 [0.677, 0.726] | 16.00 [16.00, 16.00] | — |
| none | half1_half64 | 8 | uscl | 5060 [5036, 5264] | 92472 [88668, 95890] | 1.000 [1.000, 1.000] | 0.829 [0.817, 0.831] | 0.504 [0.503, 0.505] | 3.94 [3.94, 3.95] | — |
| none | half1_half64 | 8 | fc | 14930 [10875, 16639] | 488248 [354477, 544232] | 0.831 [0.830, 0.850] | 1.000 [0.999, 1.000] | 0.726 [0.709, 0.726] | 16.00 [16.00, 16.00] | — |
| none | half1_half64 | 8 | fc_pq | 15674 [12716, 17598] | 340093 [276528, 428548] | 0.964 [0.952, 0.977] | 0.873 [0.853, 0.916] | 0.566 [0.550, 0.600] | 16.00 [16.00, 16.00] | 0.000 [0.000, 0.000] |
| immediate | all1 | 1 | native | 15516 [14628, 15697] | 15516 [14628, 15697] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.16 [1.09, 1.18] | — |
| immediate | all1 | 1 | refactored | 15664 [15648, 15786] | 15664 [15648, 15786] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.16 [1.15, 1.17] | — |
| immediate | all1 | 1 | bridge_mutex | 15744 [15742, 15848] | 15744 [15742, 15848] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.16 [1.16, 1.17] | — |
| immediate | all1 | 1 | mcs | 14566 [14120, 15746] | 14566 [14120, 15746] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.05 [1.00, 1.17] | — |
| immediate | all1 | 1 | uscl | 15704 [14972, 15718] | 15704 [14972, 15718] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.17 [1.09, 1.18] | — |
| immediate | all1 | 1 | fc | 15489 [15167, 15634] | 15489 [15167, 15634] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.14 [1.11, 1.16] | — |
| immediate | all1 | 1 | fc_pq | 15760 [15365, 15985] | 15760 [15365, 15985] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.16 [1.16, 1.22] | 1.000 [1.000, 1.000] |
| immediate | all1 | 2 | native | 15196 [15140, 15317] | 15196 [15140, 15317] | 0.730 [0.531, 0.895] | 0.750 [0.534, 0.886] | — | 1.30 [1.30, 1.37] | — |
| immediate | all1 | 2 | refactored | 15156 [14949, 15431] | 15156 [14949, 15431] | 0.956 [0.947, 0.995] | 0.968 [0.963, 0.999] | — | 1.29 [1.25, 1.33] | — |
| immediate | all1 | 2 | bridge_mutex | 15350 [14948, 15630] | 15350 [14948, 15630] | 0.876 [0.860, 0.996] | 0.883 [0.854, 0.995] | — | 1.46 [1.41, 1.47] | — |
| immediate | all1 | 2 | mcs | 14466 [14288, 14918] | 14466 [14288, 14918] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 3.22 [3.22, 3.26] | — |
| immediate | all1 | 2 | uscl | 16050 [15873, 16522] | 16050 [15873, 16522] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 3.20 [3.18, 3.22] | — |
| immediate | all1 | 2 | fc | 16196 [15920, 16286] | 16196 [15920, 16286] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 3.20 [3.20, 3.24] | — |
| immediate | all1 | 2 | fc_pq | 15997 [15600, 16064] | 15997 [15600, 16064] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 3.19 [3.18, 3.23] | 0.176 [0.175, 0.184] |
| immediate | all1 | 4 | native | 10960 [10923, 15418] | 10960 [10923, 15418] | 0.837 [0.608, 0.879] | 0.832 [0.611, 0.890] | — | 1.74 [1.36, 1.78] | — |
| immediate | all1 | 4 | refactored | 11076 [11015, 14916] | 11076 [11015, 14916] | 0.626 [0.250, 0.793] | 0.625 [0.250, 0.787] | — | 1.71 [1.30, 1.76] | — |
| immediate | all1 | 4 | bridge_mutex | 11120 [10991, 15156] | 11120 [10991, 15156] | 0.817 [0.488, 0.902] | 0.807 [0.494, 0.901] | — | 1.67 [1.47, 1.70] | — |
| immediate | all1 | 4 | mcs | 9563 [9555, 10974] | 9563 [9555, 10974] | 1.000 [0.999, 1.000] | 1.000 [1.000, 1.000] | — | 7.53 [7.45, 7.55] | — |
| immediate | all1 | 4 | uscl | 10843 [10814, 10990] | 10843 [10814, 10990] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 3.42 [3.40, 3.44] | — |
| immediate | all1 | 4 | fc | 15453 [11006, 15734] | 15453 [11006, 15734] | 0.999 [0.999, 0.999] | 0.999 [0.999, 1.000] | — | 7.25 [7.23, 7.41] | — |
| immediate | all1 | 4 | fc_pq | 11046 [11008, 11634] | 11046 [11008, 11634] | 0.995 [0.992, 0.996] | 0.991 [0.987, 0.993] | — | 7.42 [7.36, 7.42] | 0.000 [0.000, 0.000] |
| immediate | all1 | 8 | native | 11030 [11006, 11036] | 11030 [11006, 11036] | 0.637 [0.534, 0.716] | 0.615 [0.534, 0.718] | — | 1.89 [1.87, 1.89] | — |
| immediate | all1 | 8 | refactored | 14596 [10740, 15412] | 14596 [10740, 15412] | 0.244 [0.223, 0.853] | 0.248 [0.221, 0.842] | — | 1.39 [1.34, 1.81] | — |
| immediate | all1 | 8 | bridge_mutex | 10678 [10660, 14692] | 10678 [10660, 14692] | 0.753 [0.319, 0.866] | 0.742 [0.308, 0.856] | — | 1.79 [1.58, 1.80] | — |
| immediate | all1 | 8 | mcs | 9788 [9574, 12540] | 9788 [9574, 12540] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 15.53 [15.38, 15.55] | — |
| immediate | all1 | 8 | uscl | 9715 [9603, 14597] | 9715 [9603, 14597] | 1.000 [1.000, 1.000] | 0.999 [0.998, 1.000] | — | 3.42 [3.30, 3.46] | — |
| immediate | all1 | 8 | fc | 11166 [10948, 13571] | 11166 [10948, 13571] | 0.999 [0.997, 0.999] | 1.000 [0.999, 1.000] | — | 15.42 [15.40, 15.44] | — |
| immediate | all1 | 8 | fc_pq | 11258 [11192, 11278] | 11258 [11192, 11278] | 0.999 [0.998, 1.000] | 0.999 [0.997, 0.999] | — | 15.41 [15.40, 15.42] | 0.000 [0.000, 0.000] |
| immediate | half1_half64 | 1 | native | 8616 [6968, 8714] | 279988 [226428, 283173] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.22 [0.96, 1.23] | — |
| immediate | half1_half64 | 1 | refactored | 8716 [7326, 9166] | 283270 [238096, 297895] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.14 [0.97, 1.20] | — |
| immediate | half1_half64 | 1 | bridge_mutex | 7288 [7184, 9062] | 236828 [233480, 294516] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.18 [0.93, 1.27] | — |
| immediate | half1_half64 | 1 | mcs | 8547 [7236, 9164] | 277778 [235138, 297830] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.14 [0.95, 1.20] | — |
| immediate | half1_half64 | 1 | uscl | 7964 [7312, 8924] | 258798 [237640, 290030] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.19 [0.96, 1.29] | — |
| immediate | half1_half64 | 1 | fc | 7466 [7274, 9158] | 242613 [236373, 297603] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.18 [0.96, 1.31] | — |
| immediate | half1_half64 | 1 | fc_pq | 8942 [7270, 9170] | 290583 [236243, 297993] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.18 [0.96, 1.20] | 1.000 [1.000, 1.000] |
| immediate | half1_half64 | 2 | native | 6124 [6106, 9278] | 387004 [224329, 391872] | 0.506 [0.500, 0.949] | 0.510 [0.500, 0.935] | 0.994 [0.616, 1.000] | 1.29 [1.21, 1.32] | — |
| immediate | half1_half64 | 2 | refactored | 11935 [6382, 15125] | 23054 [15786, 408480] | 0.503 [0.500, 0.527] | 0.501 [0.500, 0.515] | 0.027 [0.003, 1.000] | 1.25 [1.05, 1.29] | — |
| immediate | half1_half64 | 2 | bridge_mutex | 7070 [6108, 12435] | 343584 [129678, 390944] | 0.666 [0.500, 0.855] | 0.671 [0.500, 0.793] | 0.854 [0.294, 1.000] | 1.35 [1.32, 1.35] | — |
| immediate | half1_half64 | 2 | mcs | 8529 [8431, 8530] | 277192 [274008, 277226] | 0.872 [0.865, 0.880] | 1.000 [1.000, 1.000] | 0.691 [0.685, 0.698] | 3.24 [3.24, 3.26] | — |
| immediate | half1_half64 | 2 | uscl | 10775 [8676, 10818] | 208418 [205362, 212249] | 1.000 [1.000, 1.000] | 0.858 [0.850, 0.927] | 0.502 [0.502, 0.503] | 3.20 [3.19, 3.32] | — |
| immediate | half1_half64 | 2 | fc | 8596 [8239, 8714] | 278708 [265468, 282544] | 0.883 [0.882, 0.924] | 1.000 [1.000, 1.000] | 0.682 [0.644, 0.683] | 3.18 [3.14, 3.21] | — |
| immediate | half1_half64 | 2 | fc_pq | 9580 [8952, 9940] | 245956 [236508, 258538] | 0.981 [0.979, 0.985] | 0.959 [0.955, 0.964] | 0.570 [0.562, 0.573] | 3.24 [3.20, 3.24] | 0.201 [0.185, 0.210] |
| immediate | half1_half64 | 4 | native | 14206 [4558, 14630] | 19576 [14206, 291744] | 0.359 [0.250, 0.486] | 0.348 [0.250, 0.490] | 0.016 [0.000, 1.000] | 1.28 [0.97, 1.31] | — |
| immediate | half1_half64 | 4 | refactored | 6039 [3125, 6152] | 251518 [190582, 393728] | 0.352 [0.256, 0.397] | 0.390 [0.275, 0.461] | 0.988 [0.824, 1.000] | 1.12 [1.02, 1.32] | — |
| immediate | half1_half64 | 4 | bridge_mutex | 6208 [4772, 10123] | 305376 [151400, 397280] | 0.250 [0.250, 0.574] | 0.250 [0.250, 0.389] | 1.000 [0.492, 1.000] | 1.40 [1.00, 1.51] | — |
| immediate | half1_half64 | 4 | mcs | 6807 [5744, 7937] | 221228 [186680, 257984] | 0.895 [0.866, 0.930] | 1.000 [1.000, 1.000] | 0.669 [0.593, 0.696] | 7.33 [6.99, 7.45] | — |
| immediate | half1_half64 | 4 | uscl | 7840 [6056, 9994] | 119350 [59480, 174708] | 1.000 [1.000, 1.000] | 0.769 [0.658, 0.814] | 0.502 [0.502, 0.503] | 3.27 [3.01, 3.36] | — |
| immediate | half1_half64 | 4 | fc | 5868 [5295, 8319] | 188095 [171363, 275754] | 0.888 [0.869, 0.913] | 0.999 [0.999, 0.999] | 0.676 [0.654, 0.693] | 7.10 [6.83, 7.28] | — |
| immediate | half1_half64 | 4 | fc_pq | 8784 [7030, 9364] | 194539 [184407, 197513] | 1.000 [0.943, 1.000] | 0.903 [0.884, 0.962] | 0.506 [0.496, 0.622] | 7.33 [6.94, 7.37] | 0.000 [0.000, 0.000] |
| immediate | half1_half64 | 8 | native | 3811 [2829, 4662] | 243904 [122529, 293390] | 0.126 [0.125, 0.314] | 0.129 [0.125, 0.281] | 0.996 [0.715, 1.000] | 1.46 [0.59, 1.56] | — |
| immediate | half1_half64 | 8 | refactored | 13011 [3103, 13243] | 59001 [21086, 107431] | 0.199 [0.178, 0.312] | 0.197 [0.140, 0.428] | 0.183 [0.025, 0.886] | 1.30 [0.73, 1.37] | — |
| immediate | half1_half64 | 8 | bridge_mutex | 13149 [11752, 15043] | 46570 [15043, 105213] | 0.198 [0.179, 0.201] | 0.160 [0.142, 0.205] | 0.144 [0.000, 0.245] | 1.54 [1.43, 1.55] | — |
| immediate | half1_half64 | 8 | mcs | 7849 [6631, 8073] | 255092 [215476, 262372] | 0.862 [0.860, 0.876] | 1.000 [1.000, 1.000] | 0.692 [0.687, 0.701] | 15.34 [15.33, 15.48] | — |
| immediate | half1_half64 | 8 | uscl | 8192 [3522, 8320] | 93494 [62112, 94283] | 1.000 [0.990, 1.000] | 0.690 [0.689, 0.814] | 0.503 [0.501, 0.517] | 3.52 [1.62, 3.53] | — |
| immediate | half1_half64 | 8 | fc | 7054 [4850, 7736] | 232594 [152458, 251672] | 0.886 [0.883, 0.927] | 0.999 [0.996, 1.000] | 0.678 [0.524, 0.681] | 15.35 [9.84, 15.45] | — |
| immediate | half1_half64 | 8 | fc_pq | 6548 [5642, 9185] | 156740 [111797, 211856] | 0.970 [0.965, 0.992] | 0.916 [0.859, 0.928] | 0.584 [0.542, 0.595] | 15.24 [15.10, 15.28] | 0.000 [0.000, 0.000] |

### Refactored vs native control

Median tx/s ratio refactored/native per cell against the larger relative repeat
range of the two variants (the noise).

| durability | cohort | clients | refactored/native (median tx/s) | noise range | verdict |
|---|---|---|---|---|---|
| none | all1 | 1 | 1.024 | 0.068 | within noise |
| none | all1 | 2 | 1.026 | 0.032 | within noise |
| none | all1 | 4 | 1.373 | 0.955 | within noise |
| none | all1 | 8 | 1.161 | 2.408 | within noise |
| none | half1_half64 | 1 | 1.064 | 0.008 | outside noise |
| none | half1_half64 | 2 | 1.093 | 1.816 | within noise |
| none | half1_half64 | 4 | 0.839 | 0.617 | within noise |
| none | half1_half64 | 8 | 0.919 | 2.107 | within noise |
| immediate | all1 | 1 | 1.010 | 0.069 | within noise |
| immediate | all1 | 2 | 0.997 | 0.032 | within noise |
| immediate | all1 | 4 | 1.011 | 0.410 | within noise |
| immediate | all1 | 8 | 1.323 | 0.320 | outside noise |
| immediate | half1_half64 | 1 | 1.012 | 0.211 | within noise |
| immediate | half1_half64 | 2 | 1.949 | 0.733 | outside noise |
| immediate | half1_half64 | 4 | 0.425 | 0.709 | within noise |
| immediate | half1_half64 | 8 | 3.414 | 0.779 | outside noise |

At 1 client (the only uncontended control) the ratio is 1.024 and 1.010 for
`all1` and 1.064 and 1.012 for `half1_half64` (None, Immediate). The one
outside-noise value, None `half1_half64` at 1 client (+6.4 % against a 0.8 %
spread), says patched redb is a few percent faster than upstream when
uncontended [INFERENCE: build/inlining difference; the body is compiled inside
the redb crate, while native calls redb's public API from the harness]. With 2 or
more clients both variants use redb's Mutex/Condvar, which lets one client take
the write path for long stretches (native/refactored `service_jain` near 1/c in
the half cohorts). Their tx/s then depends on which client wins, the repeat
ranges reach 2.4×, and 3 of the 12 multi-client checks are outside noise
(Immediate `all1` 8c 1.32, `half1_half64` 2c 1.95, 8c 3.41). Beyond 1 client
the control carries no information about build overhead.

### Smoke reading

The smoke is the acceptance and control artifact of the original task, and it
is kept as recorded. Its fairness order, fast-path hit rates and U-SCL cost
reappear in the follow-up formal run below. That run has more cells but the same
3 repetitions, and its absolute throughput at 4-8 clients is
frequency-confounded.

### Anomalies (unresolved)

- **Bimodal `all1` None at 4-8 clients** (diagnosed below as CPU frequency). FC/FC-PQ run either ≈31.5k or ≈20.5k
  tx/s, and MCS once 26.4k, otherwise ≈15k. Adjacent cells of the same repetition differ, and
  utilization is 0.98 in both modes, so the per-body cost changes between
  processes. Cause, diagnosed after the smoke: per-core, per-run CPU frequency
  (turbo vs base; see the follow-up section).
- **Host load spike at the end.** The 1-minute load was 59 at 07:08:44 (3.5 at
  the start, 13.9 at 07:10). No CPU consumer was visible afterwards and IO
  pressure was low. The last cells, repetition 2 of Immediate `half1_half64` at 8
  clients (07:08:30-44), are below repetitions 0-1: FC-PQ 111,797 vs
  156,740/211,856 records/s, FC 152k vs 251k/233k, and U-SCL utilization 0.69.
  Treat them as possibly disturbed. The source was not identified; it may be
  another agent's work outside the measurement lock [INFERENCE].

## Post-run documentation fixes

After the smoke, the README and this plan were corrected on waiting: MCS, FC and
FC-PQ spin; native, refactored and bridge_mutex block; U-SCL may yield or sleep.
The same wording fix in `run.py` (its docstring, the caveat and the
"fast-path hit rate" column label) was held back so that the smoke's runner hash
stayed valid. It went in with the follow-up below, before the formal root was
prepared. The smoke root's `summary.json` therefore keeps the old caveat text;
the formal and perf analyses carry the corrected one.

## Follow-up (approved 2026-09-28): cache counters and the formal matrix

Changes, all in `integration/redb/**`:

- `run.py --perf` / `--analyze-perf`: the separate cache-counter profile cohort
  (see README "Cache-counter profile cohort"). The harness now honours
  `REDB_PERF_CONTROL` (`src/main.rs`, `PerfControl`), so perf counts only the
  client phase. Timed cells never set it; the result field `perf_counted`
  checks this per cell.
- `run.py` text: the docstring and caveat now describe waiting per lock (no
  "spin-only"), and the column label is "fast-path hit rate". The fix went in
  before the formal root was prepared, so the manifest's runner hash is the
  committed one.
- `build.py`: `cargo metadata` without `--offline`.

Verification of the new build `.worktree/redb-build-03`: gate **64/64**,
upstream redb tests **165 passed** (patched tree `94132b5f…` unchanged). One
probe cell under perf control was enabled for 8.0 CPU-s = 4 clients × 2 s.

Runs, all in one prepared root `.worktree/redb-formal-01` (CPUs 16-23, node 0,
membind 0, exclusive `MEASUREMENT_LOCK`; cores 16-23 were ≤ 2 % busy before):

- perf cohort: 168 cells, 07:47:34-07:53:43, **0 failed**, no event
  multiplexed (the parser rejects < 100 % running);
- formal matrix: **504 cells**, 07:54:15-08:12:01, **0 failed**;
- `--analyze-perf` and `--analyze-only` completed. Load average was 2.2 at the
  start and 6.5 at the end.

### Root cause of the `all1` two-level swing (HardProblemSolver)

HardProblemSolver diagnosed it from the perf cohort and from live
`cpuinfo_avg_freq` reads during the formal run. The cause is **CPU clock
frequency**, not redb or the harness.

- The pinned cores run at either base 2.2 GHz or turbo (3.0-3.5 GHz) for a
  whole process. This is decided per core and per run and is stable for the 2 s.
- Cycles and instructions per transaction are invariant between the two modes:
  fast FC-PQ c4 30.0 µs per body at 3.33 GHz, slow 54.2 µs at 2.03 GHz;
  30.0 µs × 3.33 GHz ≈ 45.4 µs × 2.10 GHz ≈ 100k cycles.
- Host governor: intel_pstate passive + `schedutil`, turbo on, no HWP. Root is
  not available to pin the frequency.

Consequences for this report:

- **Every absolute tx/s and per-body µs at 4-8 clients is frequency-confounded.**
  This is labelled, not corrected. The `all1` 4-8 client cells showed it most
  and are marked **frequency-confounded (diagnosed)**.
- Within a cell, cores can differ, e.g. cpu16/17 at 3.4 GHz while cpu18/19 were
  at 2.2 GHz. [INFERENCE] For requester-run locks (MCS, U-SCL, the Mutex
  controls) a client on a slow core accrues more service time for the same
  work, which can move `service_jain`. For FC/FC-PQ it is the combiner's core
  that matters.
- The Mutex/Condvar controls' mid-run collapses are a different effect: the
  lock is monopolised by one client or handed off between sleeping clients.
  **Corrected by perf-02:** the hand-off pattern sets the clock. With
  rotation every core is mostly idle and runs at 0.87-0.90 GHz, and at 2.2 GHz
  the collapses shrink to 2-7 %.
- The fix for a future run was to record `ref-cycles` alongside `cycles` or
  per-core `cpuinfo_avg_freq` per cell, then normalise or reject cells. It is
  implemented for the perf cohort in [perf-02](./redb-perf-02-clock.md).

## Formal

**Formal matrix, 3 repetitions**, median [min, max]. Every cell is a fresh
process with exact-content verification and close/reopen. This is the
follow-up run. The Reading below is based on it, and the smoke above stays the
acceptance/control artifact. Absolute tx/s and per-body time at 4-8 clients are
**frequency-confounded (not normalised)**, so treat them as exploratory. Ratios
within a cell (service_jain, service shares, hit rates) are less exposed but not
immune: cores within a cell can run at different clocks.

| durability | cohort | clients | variant | tx/s | records/s | service_jain | tx_jain | long svc share | CPU-s | fast-path hit rate |
|---|---|---|---|---|---|---|---|---|---|---|
| none | all1 | 1 | native | 31826 [23012, 32198] | 31826 [23012, 32198] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | refactored | 33020 [30720, 33485] | 33020 [30720, 33485] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | bridge_mutex | 33016 [32210, 33025] | 33016 [32210, 33025] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | mcs | 32996 [29536, 33596] | 32996 [29536, 33596] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | uscl | 33100 [30866, 33100] | 33100 [30866, 33100] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | fc | 33128 [31090, 33344] | 33128 [31090, 33344] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | all1 | 1 | fc_pq | 33136 [30550, 33190] | 33136 [30550, 33190] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | 1.000 [1.000, 1.000] |
| none | all1 | 2 | native | 30248 [28955, 30604] | 30248 [28955, 30604] | 0.500 [0.500, 0.962] | 0.500 [0.500, 0.955] | — | 2.15 [2.13, 2.15] | — |
| none | all1 | 2 | refactored | 31138 [29536, 31341] | 31138 [29536, 31341] | 0.500 [0.500, 0.850] | 0.500 [0.500, 0.868] | — | 2.15 [2.13, 2.15] | — |
| none | all1 | 2 | bridge_mutex | 30434 [28834, 30708] | 30434 [28834, 30708] | 0.885 [0.500, 1.000] | 0.896 [0.500, 0.999] | — | 2.22 [2.21, 2.22] | — |
| none | all1 | 2 | mcs | 26452 [23268, 27306] | 26452 [23268, 27306] | 1.000 [0.995, 1.000] | 1.000 [1.000, 1.000] | — | 4.00 [4.00, 4.00] | — |
| none | all1 | 2 | uscl | 28822 [28568, 31846] | 28822 [28568, 31846] | 1.000 [1.000, 1.000] | 1.000 [0.997, 1.000] | — | 3.99 [3.98, 4.00] | — |
| none | all1 | 2 | fc | 31762 [30281, 32038] | 31762 [30281, 32038] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 4.00 [4.00, 4.00] | — |
| none | all1 | 2 | fc_pq | 31360 [29782, 31846] | 31360 [29782, 31846] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 4.00 [4.00, 4.00] | 0.181 [0.176, 0.191] |
| none | all1 | 4 | native | 11834 [7833, 19685] | 11834 [7833, 19685] | 0.791 [0.250, 0.962] | 0.498 [0.250, 0.930] | — | 2.20 [2.15, 2.29] | — |
| none | all1 | 4 | refactored | 11972 [8394, 30029] | 11972 [8394, 30029] | 0.935 [0.456, 0.981] | 0.772 [0.452, 0.902] | — | 2.18 [2.18, 2.28] | — |
| none | all1 | 4 | bridge_mutex | 19743 [9223, 25332] | 19743 [9223, 25332] | 0.500 [0.267, 0.950] | 0.486 [0.270, 0.848] | — | 2.31 [2.22, 2.39] | — |
| none | all1 | 4 | mcs | 16809 [13459, 25037] | 16809 [13459, 25037] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 8.00 [8.00, 8.00] | — |
| none | all1 | 4 | uscl | 9473 [9108, 11416] | 9473 [9108, 11416] | 1.000 [1.000, 1.000] | 0.996 [0.994, 0.998] | — | 3.95 [3.94, 3.97] | — |
| none | all1 | 4 | fc | 19958 [18203, 27254] | 19958 [18203, 27254] | 0.999 [0.997, 0.999] | 1.000 [1.000, 1.000] | — | 8.00 [8.00, 8.00] | — |
| none | all1 | 4 | fc_pq | 22792 [19884, 25726] | 22792 [19884, 25726] | 0.998 [0.997, 1.000] | 0.993 [0.984, 0.999] | — | 8.00 [8.00, 8.00] | 0.000 [0.000, 0.000] |
| none | all1 | 8 | native | 10811 [8619, 26298] | 10811 [8619, 26298] | 0.876 [0.160, 0.949] | 0.835 [0.145, 0.850] | — | 2.23 [2.21, 2.27] | — |
| none | all1 | 8 | refactored | 27732 [8671, 28016] | 27732 [8671, 28016] | 0.300 [0.131, 0.944] | 0.290 [0.129, 0.839] | — | 2.21 [2.21, 2.27] | — |
| none | all1 | 8 | bridge_mutex | 8558 [8495, 27772] | 8558 [8495, 27772] | 0.931 [0.130, 0.958] | 0.772 [0.131, 0.846] | — | 2.35 [2.33, 2.49] | — |
| none | all1 | 8 | mcs | 16890 [16560, 22872] | 16890 [16560, 22872] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 16.00 [16.00, 16.00] | — |
| none | all1 | 8 | uscl | 8109 [7939, 8582] | 8109 [7939, 8582] | 1.000 [1.000, 1.000] | 0.999 [0.988, 0.999] | — | 3.94 [3.93, 3.95] | — |
| none | all1 | 8 | fc | 24920 [19733, 28202] | 24920 [19733, 28202] | 0.998 [0.997, 0.999] | 1.000 [1.000, 1.000] | — | 16.00 [16.00, 16.00] | — |
| none | all1 | 8 | fc_pq | 27557 [20478, 29226] | 27557 [20478, 29226] | 0.999 [0.999, 0.999] | 0.998 [0.998, 0.999] | — | 16.00 [16.00, 16.00] | 0.000 [0.000, 0.000] |
| none | half1_half8 | 1 | native | 27934 [26704, 28253] | 125699 [120168, 127138] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half8 | 1 | refactored | 29030 [29012, 29038] | 130635 [130554, 130671] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half8 | 1 | bridge_mutex | 29324 [29256, 29575] | 131954 [131652, 133088] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half8 | 1 | mcs | 29284 [29162, 29502] | 131774 [131230, 132759] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half8 | 1 | uscl | 29072 [23773, 29358] | 130824 [106978, 132107] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half8 | 1 | fc | 28746 [23436, 29409] | 129357 [105458, 132340] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half8 | 1 | fc_pq | 29124 [17480, 29288] | 131054 [78660, 131792] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | 1.000 [1.000, 1.000] |
| none | half1_half8 | 2 | native | 24358 [24208, 31965] | 175716 [31965, 194860] | 0.500 [0.500, 0.616] | 0.500 [0.500, 0.617] | 0.895 [0.000, 1.000] | 2.09 [2.07, 2.12] | — |
| none | half1_half8 | 2 | refactored | 24782 [22264, 26402] | 178108 [162040, 198252] | 0.500 [0.500, 0.761] | 0.500 [0.500, 0.820] | 1.000 [0.780, 1.000] | 2.12 [2.11, 2.13] | — |
| none | half1_half8 | 2 | bridge_mutex | 27315 [25296, 30895] | 166044 [46071, 173996] | 0.652 [0.587, 0.802] | 0.684 [0.575, 0.831] | 0.749 [0.081, 0.865] | 2.18 [2.18, 2.23] | — |
| none | half1_half8 | 2 | mcs | 23704 [23107, 23980] | 106672 [103982, 107914] | 0.995 [0.988, 0.995] | 1.000 [1.000, 1.000] | 0.537 [0.534, 0.554] | 4.00 [4.00, 4.00] | — |
| none | half1_half8 | 2 | uscl | 28318 [27357, 29574] | 119486 [114962, 124816] | 1.000 [1.000, 1.000] | 0.994 [0.993, 0.994] | 0.503 [0.500, 0.504] | 3.99 [3.97, 4.00] | — |
| none | half1_half8 | 2 | fc | 28288 [26990, 28399] | 127285 [121424, 127796] | 0.994 [0.992, 0.995] | 1.000 [1.000, 1.000] | 0.538 [0.537, 0.544] | 4.00 [4.00, 4.00] | — |
| none | half1_half8 | 2 | fc_pq | 28234 [27461, 28878] | 119378 [116333, 121908] | 1.000 [1.000, 1.000] | 0.994 [0.994, 0.994] | 0.500 [0.500, 0.501] | 4.00 [4.00, 4.00] | 0.165 [0.149, 0.167] |
| none | half1_half8 | 4 | native | 28840 [23029, 31396] | 28840 [26795, 31396] | 0.250 [0.250, 0.530] | 0.250 [0.250, 0.500] | 0.000 [0.000, 0.029] | 2.16 [2.15, 2.19] | — |
| none | half1_half8 | 4 | refactored | 26496 [20096, 26612] | 160772 [26612, 211964] | 0.401 [0.250, 0.431] | 0.353 [0.250, 0.446] | 1.000 [0.000, 1.000] | 2.18 [2.15, 2.19] | — |
| none | half1_half8 | 4 | bridge_mutex | 26653 [16210, 31952] | 31952 [26653, 129676] | 0.366 [0.271, 0.499] | 0.376 [0.258, 0.499] | 0.000 [0.000, 1.000] | 2.44 [2.28, 2.48] | — |
| none | half1_half8 | 4 | mcs | 14749 [14580, 17846] | 66370 [65614, 80308] | 0.994 [0.965, 0.995] | 1.000 [1.000, 1.000] | 0.537 [0.536, 0.540] | 8.00 [8.00, 8.00] | — |
| none | half1_half8 | 4 | uscl | 7688 [7622, 8102] | 30784 [29651, 33046] | 1.000 [1.000, 1.000] | 0.976 [0.970, 0.984] | 0.501 [0.501, 0.502] | 3.94 [3.93, 3.97] | — |
| none | half1_half8 | 4 | fc | 20324 [16810, 25336] | 90758 [76204, 115272] | 0.988 [0.986, 0.995] | 1.000 [0.999, 1.000] | 0.554 [0.532, 0.558] | 8.00 [8.00, 8.00] | — |
| none | half1_half8 | 4 | fc_pq | 17092 [16866, 21990] | 73374 [71090, 102714] | 1.000 [0.999, 1.000] | 0.998 [0.991, 0.998] | 0.506 [0.505, 0.512] | 8.00 [8.00, 8.00] | 0.000 [0.000, 0.000] |
| none | half1_half8 | 8 | native | 19898 [6646, 25664] | 26444 [19912, 45164] | 0.163 [0.140, 0.610] | 0.168 [0.150, 0.582] | 0.017 [0.000, 0.833] | 2.19 [2.15, 2.28] | — |
| none | half1_half8 | 8 | refactored | 17068 [7128, 23326] | 59194 [49215, 112545] | 0.260 [0.254, 0.673] | 0.266 [0.196, 0.646] | 0.845 [0.359, 0.856] | 2.19 [2.14, 2.27] | — |
| none | half1_half8 | 8 | bridge_mutex | 14391 [7780, 28871] | 54358 [29448, 57211] | 0.434 [0.129, 0.528] | 0.292 [0.126, 0.404] | 0.541 [0.014, 0.896] | 2.34 [2.31, 2.43] | — |
| none | half1_half8 | 8 | mcs | 14595 [14196, 14598] | 65681 [63886, 65694] | 0.995 [0.994, 0.995] | 1.000 [1.000, 1.000] | 0.536 [0.535, 0.539] | 16.00 [16.00, 16.00] | — |
| none | half1_half8 | 8 | uscl | 6653 [6486, 7094] | 27580 [27164, 29390] | 1.000 [1.000, 1.000] | 0.990 [0.989, 0.992] | 0.501 [0.501, 0.501] | 3.94 [3.93, 3.95] | — |
| none | half1_half8 | 8 | fc | 17008 [17000, 22321] | 76941 [76848, 100122] | 0.992 [0.992, 0.993] | 1.000 [1.000, 1.000] | 0.540 [0.538, 0.540] | 16.00 [16.00, 16.00] | — |
| none | half1_half8 | 8 | fc_pq | 17737 [17523, 24006] | 75330 [74860, 102252] | 0.999 [0.998, 0.999] | 0.994 [0.993, 0.994] | 0.508 [0.508, 0.510] | 16.00 [16.00, 16.00] | 0.000 [0.000, 0.000] |
| none | half1_half64 | 1 | native | 16192 [16120, 16424] | 526240 [523900, 533748] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | refactored | 17044 [17030, 17238] | 553930 [553475, 560203] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | bridge_mutex | 17266 [16844, 17630] | 561113 [547430, 572943] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | mcs | 16994 [16862, 17996] | 552306 [548016, 584838] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | uscl | 17460 [16752, 17516] | 567418 [544440, 569270] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | fc | 17096 [16936, 17129] | 555588 [550420, 556692] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | — |
| none | half1_half64 | 1 | fc_pq | 17146 [17050, 17222] | 557213 [554093, 559683] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 2.00 [2.00, 2.00] | 1.000 [1.000, 1.000] |
| none | half1_half64 | 2 | native | 18356 [10654, 29738] | 472492 [29738, 681824] | 0.500 [0.500, 0.915] | 0.500 [0.500, 0.956] | 0.652 [0.000, 1.000] | 2.08 [2.06, 2.09] | — |
| none | half1_half64 | 2 | refactored | 11580 [11327, 11724] | 741088 [724928, 750304] | 0.500 [0.500, 0.500] | 0.500 [0.500, 0.500] | 1.000 [1.000, 1.000] | 2.06 [2.06, 2.08] | — |
| none | half1_half64 | 2 | bridge_mutex | 18200 [17585, 24020] | 472524 [158462, 509174] | 0.905 [0.856, 0.957] | 0.959 [0.597, 0.987] | 0.606 [0.295, 0.662] | 2.13 [2.13, 2.17] | — |
| none | half1_half64 | 2 | mcs | 15082 [12560, 15156] | 490133 [408200, 492538] | 0.872 [0.809, 0.876] | 1.000 [1.000, 1.000] | 0.692 [0.688, 0.743] | 4.00 [4.00, 4.00] | — |
| none | half1_half64 | 2 | uscl | 20602 [20449, 20852] | 396716 [389183, 404458] | 1.000 [1.000, 1.000] | 0.852 [0.843, 0.853] | 0.502 [0.502, 0.502] | 3.98 [3.97, 3.99] | — |
| none | half1_half64 | 2 | fc | 16833 [16544, 17106] | 547072 [537112, 555882] | 0.857 [0.855, 0.858] | 1.000 [1.000, 1.000] | 0.704 [0.703, 0.706] | 4.00 [4.00, 4.00] | — |
| none | half1_half64 | 2 | fc_pq | 17887 [17591, 18651] | 437940 [433769, 468314] | 0.965 [0.965, 0.966] | 0.942 [0.939, 0.948] | 0.595 [0.594, 0.596] | 4.00 [4.00, 4.00] | 0.242 [0.224, 0.247] |
| none | half1_half64 | 4 | native | 10486 [5432, 15952] | 288396 [156016, 425514] | 0.662 [0.479, 0.797] | 0.483 [0.428, 0.696] | 0.604 [0.550, 0.857] | 2.13 [2.11, 2.16] | — |
| none | half1_half64 | 4 | refactored | 7428 [5658, 12830] | 305318 [289699, 374388] | 0.409 [0.373, 0.588] | 0.515 [0.495, 0.588] | 0.791 [0.756, 0.889] | 2.13 [2.09, 2.15] | — |
| none | half1_half64 | 4 | bridge_mutex | 11003 [5089, 16805] | 302638 [255292, 704192] | 0.528 [0.250, 0.909] | 0.495 [0.250, 0.673] | 0.944 [0.458, 1.000] | 2.18 [2.16, 2.30] | — |
| none | half1_half64 | 4 | mcs | 10874 [9380, 14130] | 353373 [304818, 459193] | 0.879 [0.792, 0.882] | 1.000 [1.000, 1.000] | 0.686 [0.683, 0.756] | 8.00 [8.00, 8.00] | — |
| none | half1_half64 | 4 | uscl | 5361 [5124, 5487] | 95844 [92994, 98286] | 1.000 [1.000, 1.000] | 0.814 [0.809, 0.837] | 0.504 [0.504, 0.504] | 3.94 [3.93, 3.97] | — |
| none | half1_half64 | 4 | fc | 10326 [9750, 15174] | 338556 [313284, 499865] | 0.842 [0.839, 0.874] | 0.999 [0.999, 1.000] | 0.716 [0.687, 0.719] | 8.00 [8.00, 8.00] | — |
| none | half1_half64 | 4 | fc_pq | 12873 [12866, 13018] | 246099 [235666, 250182] | 1.000 [0.999, 1.000] | 0.847 [0.831, 0.849] | 0.509 [0.497, 0.518] | 8.00 [8.00, 8.00] | 0.000 [0.000, 0.000] |
| none | half1_half64 | 8 | native | 10330 [5778, 28058] | 219411 [28058, 661120] | 0.136 [0.125, 0.670] | 0.138 [0.125, 0.660] | 0.830 [0.000, 1.000] | 2.19 [2.10, 2.21] | — |
| none | half1_half64 | 8 | refactored | 8474 [5464, 10688] | 308858 [248486, 684000] | 0.498 [0.125, 0.502] | 0.432 [0.125, 0.582] | 0.880 [0.804, 1.000] | 2.16 [2.10, 2.19] | — |
| none | half1_half64 | 8 | bridge_mutex | 4100 [4003, 13725] | 236725 [233232, 463828] | 0.517 [0.236, 0.526] | 0.571 [0.299, 0.590] | 0.964 [0.668, 0.966] | 2.23 [2.22, 2.26] | — |
| none | half1_half64 | 8 | mcs | 9576 [8018, 10449] | 311220 [260553, 339592] | 0.872 [0.806, 0.904] | 1.000 [1.000, 1.000] | 0.691 [0.663, 0.739] | 16.00 [16.00, 16.00] | — |
| none | half1_half64 | 8 | uscl | 4902 [4852, 5657] | 91698 [90771, 110489] | 1.000 [1.000, 1.000] | 0.843 [0.835, 0.851] | 0.504 [0.504, 0.505] | 3.96 [3.95, 3.97] | — |
| none | half1_half64 | 8 | fc | 10984 [10464, 13709] | 359815 [336080, 446992] | 0.850 [0.848, 0.852] | 1.000 [1.000, 1.000] | 0.710 [0.708, 0.712] | 16.00 [16.00, 16.00] | — |
| none | half1_half64 | 8 | fc_pq | 13702 [12164, 16601] | 318087 [280324, 411359] | 0.963 [0.958, 0.971] | 0.915 [0.906, 0.940] | 0.592 [0.585, 0.604] | 16.00 [16.00, 16.00] | 0.000 [0.000, 0.000] |
| immediate | all1 | 1 | native | 14190 [12602, 14751] | 14190 [12602, 14751] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.21 [1.01, 1.24] | — |
| immediate | all1 | 1 | refactored | 14882 [14166, 14976] | 14882 [14166, 14976] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.21 [1.20, 1.24] | — |
| immediate | all1 | 1 | bridge_mutex | 14804 [14334, 14920] | 14804 [14334, 14920] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.21 [1.21, 1.23] | — |
| immediate | all1 | 1 | mcs | 14485 [12046, 14946] | 14485 [12046, 14946] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.20 [0.90, 1.21] | — |
| immediate | all1 | 1 | uscl | 14884 [14312, 14910] | 14884 [14312, 14910] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.23 [1.21, 1.26] | — |
| immediate | all1 | 1 | fc | 13972 [13604, 14874] | 13972 [13604, 14874] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.22 [1.07, 1.26] | — |
| immediate | all1 | 1 | fc_pq | 14802 [14210, 14991] | 14802 [14210, 14991] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.22 [1.11, 1.22] | 1.000 [1.000, 1.000] |
| immediate | all1 | 2 | native | 14534 [14422, 14578] | 14534 [14422, 14578] | 0.761 [0.608, 0.788] | 0.759 [0.612, 0.803] | — | 1.37 [1.35, 1.38] | — |
| immediate | all1 | 2 | refactored | 14420 [14410, 14569] | 14420 [14410, 14569] | 0.913 [0.656, 0.943] | 0.933 [0.655, 0.941] | — | 1.33 [1.31, 1.36] | — |
| immediate | all1 | 2 | bridge_mutex | 14387 [14276, 14613] | 14387 [14276, 14613] | 0.556 [0.500, 0.776] | 0.561 [0.500, 0.758] | — | 1.47 [1.46, 1.52] | — |
| immediate | all1 | 2 | mcs | 13517 [12663, 13572] | 13517 [12663, 13572] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 3.24 [3.17, 3.34] | — |
| immediate | all1 | 2 | uscl | 14612 [13462, 14890] | 14612 [13462, 14890] | 1.000 [1.000, 1.000] | 1.000 [0.980, 1.000] | — | 3.30 [3.24, 3.38] | — |
| immediate | all1 | 2 | fc | 15234 [14318, 15256] | 15234 [14318, 15256] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 3.25 [3.18, 3.30] | — |
| immediate | all1 | 2 | fc_pq | 14844 [14414, 14906] | 14844 [14414, 14906] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 3.27 [3.26, 3.32] | 0.174 [0.165, 0.181] |
| immediate | all1 | 4 | native | 14256 [10168, 14364] | 14256 [10168, 14364] | 0.619 [0.422, 0.749] | 0.596 [0.426, 0.749] | — | 1.42 [1.38, 1.75] | — |
| immediate | all1 | 4 | refactored | 14412 [11250, 14548] | 14412 [11250, 14548] | 0.491 [0.250, 0.899] | 0.498 [0.250, 0.896] | — | 1.40 [1.39, 1.77] | — |
| immediate | all1 | 4 | bridge_mutex | 14610 [10587, 14852] | 14610 [10587, 14852] | 0.389 [0.372, 0.879] | 0.399 [0.367, 0.864] | — | 1.58 [1.52, 1.73] | — |
| immediate | all1 | 4 | mcs | 10804 [8793, 11276] | 10804 [8793, 11276] | 0.999 [0.999, 1.000] | 1.000 [1.000, 1.000] | — | 7.47 [7.46, 7.58] | — |
| immediate | all1 | 4 | uscl | 11038 [10311, 14062] | 11038 [10311, 14062] | 1.000 [1.000, 1.000] | 1.000 [0.991, 1.000] | — | 3.43 [3.34, 3.48] | — |
| immediate | all1 | 4 | fc | 14374 [14013, 14668] | 14374 [14013, 14668] | 0.999 [0.999, 0.999] | 0.999 [0.999, 0.999] | — | 7.31 [7.29, 7.33] | — |
| immediate | all1 | 4 | fc_pq | 11502 [10750, 14423] | 11502 [10750, 14423] | 0.996 [0.973, 0.997] | 0.996 [0.968, 0.997] | — | 7.48 [7.31, 7.49] | 0.000 [0.000, 0.000] |
| immediate | all1 | 8 | native | 11252 [11244, 11682] | 11252 [11244, 11682] | 0.698 [0.138, 0.708] | 0.668 [0.139, 0.682] | — | 1.85 [1.64, 1.87] | — |
| immediate | all1 | 8 | refactored | 11426 [10700, 14426] | 11426 [10700, 14426] | 0.614 [0.266, 0.760] | 0.597 [0.274, 0.739] | — | 1.82 [1.43, 1.84] | — |
| immediate | all1 | 8 | bridge_mutex | 11067 [10858, 12267] | 11067 [10858, 12267] | 0.904 [0.427, 0.929] | 0.891 [0.448, 0.924] | — | 1.80 [1.68, 1.81] | — |
| immediate | all1 | 8 | mcs | 10668 [10612, 13308] | 10668 [10612, 13308] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 15.50 [15.39, 15.53] | — |
| immediate | all1 | 8 | uscl | 10199 [10168, 10964] | 10199 [10168, 10964] | 1.000 [1.000, 1.000] | 0.995 [0.995, 0.998] | — | 3.49 [3.47, 3.49] | — |
| immediate | all1 | 8 | fc | 11597 [11436, 12130] | 11597 [11436, 12130] | 0.999 [0.999, 1.000] | 1.000 [0.999, 1.000] | — | 15.49 [15.47, 15.50] | — |
| immediate | all1 | 8 | fc_pq | 11721 [11612, 12962] | 11721 [11612, 12962] | 0.998 [0.998, 0.999] | 0.998 [0.998, 0.999] | — | 15.47 [15.42, 15.47] | 0.000 [0.000, 0.000] |
| immediate | half1_half8 | 1 | native | 13415 [13180, 13966] | 60368 [59306, 62848] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.28 [1.23, 1.32] | — |
| immediate | half1_half8 | 1 | refactored | 14086 [13857, 14170] | 63383 [62356, 63766] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.28 [1.24, 1.29] | — |
| immediate | half1_half8 | 1 | bridge_mutex | 13775 [13502, 14002] | 61988 [60759, 63005] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.26 [1.22, 1.27] | — |
| immediate | half1_half8 | 1 | mcs | 13700 [13633, 14488] | 61646 [61348, 65196] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.26 [1.24, 1.28] | — |
| immediate | half1_half8 | 1 | uscl | 13521 [13196, 13776] | 60844 [59382, 61992] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.27 [1.20, 1.28] | — |
| immediate | half1_half8 | 1 | fc | 13932 [13152, 14286] | 62690 [59180, 64283] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.26 [1.22, 1.26] | — |
| immediate | half1_half8 | 1 | fc_pq | 13738 [13650, 14108] | 61821 [61421, 63486] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.25 [1.24, 1.26] | 1.000 [1.000, 1.000] |
| immediate | half1_half8 | 2 | native | 12178 [11957, 14918] | 84698 [14922, 95652] | 0.500 [0.500, 0.653] | 0.500 [0.500, 0.670] | 0.864 [0.000, 1.000] | 1.36 [1.35, 1.38] | — |
| immediate | half1_half8 | 2 | refactored | 13004 [12547, 13347] | 46343 [30287, 79599] | 0.800 [0.729, 0.967] | 0.823 [0.711, 0.950] | 0.408 [0.195, 0.750] | 1.35 [1.23, 1.37] | — |
| immediate | half1_half8 | 2 | bridge_mutex | 13880 [12718, 14548] | 34660 [14548, 79466] | 0.771 [0.500, 0.792] | 0.753 [0.500, 0.800] | 0.244 [0.000, 0.772] | 1.47 [1.44, 1.47] | — |
| immediate | half1_half8 | 2 | mcs | 12542 [12325, 12856] | 56438 [55462, 57848] | 0.998 [0.998, 0.998] | 1.000 [1.000, 1.000] | 0.522 [0.522, 0.523] | 3.34 [3.31, 3.35] | — |
| immediate | half1_half8 | 2 | uscl | 13667 [13360, 13882] | 57826 [56732, 59228] | 1.000 [1.000, 1.000] | 0.995 [0.994, 0.996] | 0.501 [0.500, 0.502] | 3.28 [3.24, 3.29] | — |
| immediate | half1_half8 | 2 | fc | 13570 [12785, 13725] | 61092 [57344, 61598] | 0.998 [0.997, 0.998] | 1.000 [1.000, 1.000] | 0.523 [0.523, 0.525] | 3.27 [3.23, 3.31] | — |
| immediate | half1_half8 | 2 | fc_pq | 13006 [12496, 13663] | 56386 [53604, 59086] | 1.000 [1.000, 1.000] | 0.997 [0.996, 0.998] | 0.500 [0.500, 0.500] | 3.30 [3.19, 3.36] | 0.183 [0.172, 0.201] |
| immediate | half1_half8 | 4 | native | 11728 [9738, 12362] | 82796 [17463, 93828] | 0.337 [0.250, 0.602] | 0.360 [0.250, 0.544] | 0.849 [0.165, 1.000] | 1.41 [1.38, 1.72] | — |
| immediate | half1_half8 | 4 | refactored | 12592 [12428, 13966] | 63054 [18985, 88752] | 0.434 [0.313, 0.478] | 0.414 [0.328, 0.490] | 0.607 [0.075, 0.902] | 1.41 [1.39, 1.41] | — |
| immediate | half1_half8 | 4 | bridge_mutex | 12450 [12299, 13690] | 90531 [31152, 94616] | 0.356 [0.341, 0.711] | 0.360 [0.355, 0.682] | 0.919 [0.210, 0.955] | 1.51 [1.50, 1.54] | — |
| immediate | half1_half8 | 4 | mcs | 8826 [8445, 11612] | 39718 [38002, 52254] | 0.994 [0.993, 0.997] | 1.000 [1.000, 1.000] | 0.537 [0.526, 0.540] | 7.40 [7.38, 7.42] | — |
| immediate | half1_half8 | 4 | uscl | 11809 [10083, 12708] | 44608 [42720, 54242] | 1.000 [1.000, 1.000] | 0.994 [0.959, 0.996] | 0.501 [0.501, 0.502] | 3.43 [3.33, 3.48] | — |
| immediate | half1_half8 | 4 | fc | 11243 [9160, 11518] | 50306 [40856, 52510] | 0.996 [0.993, 0.997] | 0.999 [0.999, 0.999] | 0.530 [0.517, 0.539] | 7.41 [7.40, 7.41] | — |
| immediate | half1_half8 | 4 | fc_pq | 12674 [9840, 13146] | 48346 [42376, 56550] | 0.999 [0.972, 1.000] | 0.997 [0.948, 0.999] | 0.507 [0.415, 0.517] | 7.34 [7.29, 7.43] | 0.000 [0.000, 0.000] |
| immediate | half1_half8 | 8 | native | 11329 [10974, 14188] | 41225 [14192, 90632] | 0.243 [0.125, 0.250] | 0.239 [0.125, 0.248] | 0.417 [0.000, 1.000] | 1.42 [1.22, 1.44] | — |
| immediate | half1_half8 | 8 | refactored | 12426 [11740, 14278] | 64201 [37798, 93920] | 0.232 [0.207, 0.252] | 0.230 [0.196, 0.264] | 0.622 [0.271, 1.000] | 1.43 [1.40, 1.45] | — |
| immediate | half1_half8 | 8 | bridge_mutex | 13506 [13062, 13715] | 41582 [36744, 49724] | 0.344 [0.210, 0.351] | 0.356 [0.203, 0.359] | 0.325 [0.281, 0.442] | 1.56 [1.50, 1.57] | — |
| immediate | half1_half8 | 8 | mcs | 11038 [9342, 11683] | 49672 [42035, 52577] | 0.992 [0.978, 0.997] | 1.000 [1.000, 1.000] | 0.542 [0.528, 0.566] | 15.47 [15.41, 15.49] | — |
| immediate | half1_half8 | 8 | uscl | 10302 [10013, 11373] | 41808 [40827, 47017] | 1.000 [1.000, 1.000] | 0.983 [0.946, 0.989] | 0.500 [0.500, 0.502] | 3.36 [3.30, 3.43] | — |
| immediate | half1_half8 | 8 | fc | 11817 [11704, 12370] | 52970 [52224, 55175] | 0.996 [0.982, 0.997] | 1.000 [1.000, 1.000] | 0.524 [0.482, 0.528] | 15.36 [15.23, 15.39] | — |
| immediate | half1_half8 | 8 | fc_pq | 12777 [12465, 13012] | 55908 [54612, 56282] | 0.999 [0.999, 1.000] | 0.998 [0.997, 0.998] | 0.509 [0.503, 0.511] | 15.34 [15.34, 15.36] | 0.000 [0.000, 0.000] |
| immediate | half1_half64 | 1 | native | 8024 [7890, 8206] | 260780 [256426, 266696] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.26 [1.23, 1.29] | — |
| immediate | half1_half64 | 1 | refactored | 8508 [8252, 8586] | 276510 [268190, 279045] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.23 [1.18, 1.27] | — |
| immediate | half1_half64 | 1 | bridge_mutex | 8499 [8218, 8614] | 276218 [267053, 279923] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.23 [1.17, 1.25] | — |
| immediate | half1_half64 | 1 | mcs | 8172 [8064, 8402] | 265590 [262080, 273066] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.26 [1.18, 1.28] | — |
| immediate | half1_half64 | 1 | uscl | 8492 [8436, 8620] | 275990 [274138, 280118] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.24 [1.22, 1.25] | — |
| immediate | half1_half64 | 1 | fc | 8327 [8149, 8612] | 270628 [264842, 279890] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.20 [1.19, 1.25] | — |
| immediate | half1_half64 | 1 | fc_pq | 8478 [7796, 8500] | 275503 [253338, 276250] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.000] | — | 1.27 [1.25, 1.32] | 1.000 [1.000, 1.000] |
| immediate | half1_half64 | 2 | native | 8924 [8905, 9791] | 220428 [45228, 250560] | 0.903 [0.603, 0.981] | 0.943 [0.561, 0.981] | 0.570 [0.095, 0.664] | 1.35 [1.33, 1.55] | — |
| immediate | half1_half64 | 2 | refactored | 8400 [6028, 13740] | 249658 [14685, 385760] | 0.503 [0.500, 0.930] | 0.501 [0.500, 0.992] | 0.637 [0.003, 1.000] | 1.26 [1.22, 1.33] | — |
| immediate | half1_half64 | 2 | bridge_mutex | 6154 [6098, 11569] | 390304 [110510, 393824] | 0.500 [0.500, 0.808] | 0.500 [0.500, 0.653] | 1.000 [0.256, 1.000] | 1.39 [1.39, 1.42] | — |
| immediate | half1_half64 | 2 | mcs | 7826 [7752, 7886] | 254313 [251940, 256295] | 0.881 [0.874, 0.890] | 1.000 [1.000, 1.000] | 0.684 [0.676, 0.690] | 3.30 [3.28, 3.30] | — |
| immediate | half1_half64 | 2 | uscl | 10077 [8743, 10178] | 201440 [198468, 204155] | 1.000 [1.000, 1.000] | 0.865 [0.864, 0.912] | 0.502 [0.502, 0.502] | 3.26 [3.23, 3.31] | — |
| immediate | half1_half64 | 2 | fc | 8227 [8184, 8373] | 266201 [263377, 271713] | 0.890 [0.887, 0.891] | 1.000 [1.000, 1.000] | 0.676 [0.675, 0.679] | 3.25 [3.24, 3.25] | — |
| immediate | half1_half64 | 2 | fc_pq | 8988 [8864, 9049] | 236670 [235818, 236703] | 0.976 [0.974, 0.982] | 0.963 [0.960, 0.967] | 0.578 [0.567, 0.582] | 3.26 [3.26, 3.30] | 0.195 [0.184, 0.202] |
| immediate | half1_half64 | 4 | native | 5503 [4352, 14139] | 217404 [14139, 278560] | 0.250 [0.250, 0.506] | 0.250 [0.250, 0.589] | 0.790 [0.000, 1.000] | 1.44 [1.40, 1.53] | — |
| immediate | half1_half64 | 4 | refactored | 3112 [2992, 12298] | 176236 [79991, 182070] | 0.514 [0.348, 0.519] | 0.551 [0.297, 0.601] | 0.960 [0.169, 0.986] | 1.12 [1.10, 1.27] | — |
| immediate | half1_half64 | 4 | bridge_mutex | 6112 [2938, 10868] | 175621 [98784, 391200] | 0.432 [0.250, 0.484] | 0.322 [0.250, 0.527] | 0.978 [0.301, 1.000] | 1.38 [1.14, 1.51] | — |
| immediate | half1_half64 | 4 | mcs | 6515 [3985, 6642] | 211738 [129512, 215865] | 0.873 [0.861, 0.874] | 1.000 [1.000, 1.000] | 0.688 [0.688, 0.700] | 7.46 [7.05, 7.48] | — |
| immediate | half1_half64 | 4 | uscl | 5491 [5232, 5911] | 56552 [55317, 64942] | 1.000 [1.000, 1.000] | 0.674 [0.668, 0.681] | 0.502 [0.502, 0.502] | 3.27 [3.27, 3.29] | — |
| immediate | half1_half64 | 4 | fc | 7582 [4461, 7814] | 244431 [143754, 259876] | 0.904 [0.891, 0.908] | 0.999 [0.999, 0.999] | 0.663 [0.658, 0.670] | 7.29 [7.05, 7.34] | — |
| immediate | half1_half64 | 4 | fc_pq | 7465 [4459, 8048] | 168997 [104251, 180700] | 0.998 [0.975, 0.999] | 0.911 [0.908, 0.923] | 0.522 [0.513, 0.579] | 7.33 [7.05, 7.43] | 0.000 [0.000, 0.000] |
| immediate | half1_half64 | 8 | native | 8650 [5041, 9618] | 150872 [127135, 208918] | 0.247 [0.236, 0.307] | 0.334 [0.204, 0.358] | 0.559 [0.536, 0.727] | 1.40 [1.27, 1.45] | — |
| immediate | half1_half64 | 8 | refactored | 5856 [2820, 9518] | 179441 [135802, 374752] | 0.248 [0.125, 0.250] | 0.187 [0.125, 0.251] | 0.998 [0.492, 1.000] | 1.34 [1.05, 1.43] | — |
| immediate | half1_half64 | 8 | bridge_mutex | 8294 [2752, 8867] | 201074 [155432, 242975] | 0.232 [0.229, 0.337] | 0.244 [0.234, 0.402] | 0.649 [0.638, 0.955] | 1.43 [1.09, 1.56] | — |
| immediate | half1_half64 | 8 | mcs | 6679 [6209, 7040] | 217036 [201824, 228832] | 0.866 [0.860, 0.882] | 1.000 [1.000, 1.000] | 0.692 [0.683, 0.695] | 15.48 [15.40, 15.50] | — |
| immediate | half1_half64 | 8 | uscl | 7631 [4960, 7679] | 93202 [52682, 110510] | 1.000 [1.000, 1.000] | 0.704 [0.669, 0.745] | 0.503 [0.502, 0.503] | 3.53 [3.36, 3.54] | — |
| immediate | half1_half64 | 8 | fc | 7074 [4179, 7254] | 228582 [134022, 238968] | 0.879 [0.854, 0.908] | 0.999 [0.999, 1.000] | 0.684 [0.658, 0.705] | 15.27 [15.01, 15.41] | — |
| immediate | half1_half64 | 8 | fc_pq | 5824 [5637, 7617] | 135196 [132265, 178851] | 0.967 [0.957, 0.984] | 0.919 [0.908, 0.929] | 0.592 [0.562, 0.606] | 15.17 [15.11, 15.47] | 0.000 [0.000, 0.000] |

Refactored vs native control (median tx/s ratio; noise = larger relative repeat
range):

- **None:** at 1 client `all1` 1.038, `half1_half8` 1.039 and `half1_half64`
  1.053. Only the last is outside noise (spread 0.019). Every multi-client check
  is within noise, but the noise is 0.06-2.2.
- **Immediate:** 1.049 / 1.050 / 1.060 at 1 client. `half1_half64` (1.060) is
  outside noise. Every multi-client check is within noise.
- Patched redb is ≈4-6 % faster than upstream uncontended. The one
  outside-noise value, `half1_half64` at 1 client, repeats in both regimes.
  [INFERENCE: codegen/inlining of the body inside the redb crate.]

## Cache counters

Profile cohort (None; 168 cells; process totals over all threads, user mode;
client phase only; per committed transaction). Median over 3 repetitions;
HITM loads/tx also shows [min, max]. Its throughput is perf-affected and not
reported as a result. perf stat cannot attribute a launched command's threads
separately, so these are **process totals**: the combiner's or holder's body
work is not separated from waiters.

| cohort | clients | variant | HITM loads/tx | HITM supplied/tx | L2-miss loads/tx | L2 misses/tx | LLC misses/tx | instructions/tx | HITM loads/record | LLC misses/record |
|---|---|---|---|---|---|---|---|---|---|---|
| all1 | 1 | native | 0.0 [0.0, 0.0] | 9 | 0 | 2 | 0.9 | 222558 | 0.00 | 0.89 |
| all1 | 1 | refactored | 0.0 [0.0, 0.0] | 9 | 0 | 2 | 0.6 | 214661 | 0.00 | 0.57 |
| all1 | 1 | bridge_mutex | 0.0 [0.0, 0.0] | 9 | 0 | 2 | 0.6 | 214914 | 0.00 | 0.57 |
| all1 | 1 | mcs | 0.0 [0.0, 0.0] | 9 | 0 | 2 | 0.7 | 214530 | 0.00 | 0.68 |
| all1 | 1 | uscl | 0.0 [0.0, 0.0] | 9 | 0 | 2 | 0.8 | 214788 | 0.00 | 0.79 |
| all1 | 1 | fc | 0.0 [0.0, 0.0] | 9 | 0 | 2 | 0.7 | 215039 | 0.00 | 0.68 |
| all1 | 1 | fc_pq | 0.0 [0.0, 0.0] | 9 | 0 | 2 | 0.6 | 215275 | 0.00 | 0.64 |
| all1 | 2 | native | 1.0 [1.0, 1.0] | 18 | 2 | 10 | 0.7 | 221701 | 1.03 | 0.66 |
| all1 | 2 | refactored | 1.2 [1.1, 1.2] | 18 | 1 | 10 | 0.5 | 214987 | 1.16 | 0.53 |
| all1 | 2 | bridge_mutex | 1.0 [1.0, 1.0] | 15 | 1 | 4 | 0.5 | 215527 | 1.01 | 0.51 |
| all1 | 2 | mcs | 93.9 [90.7, 95.5] | 718 | 107 | 717 | 1.3 | 222823 | 93.91 | 1.30 |
| all1 | 2 | uscl | 2.2 [2.1, 2.4] | 15 | 3 | 22 | 0.6 | 220846 | 2.16 | 0.62 |
| all1 | 2 | fc | 8.2 [7.7, 8.2] | 63 | 9 | 45 | 0.6 | 217574 | 8.18 | 0.61 |
| all1 | 2 | fc_pq | 11.3 [10.8, 11.6] | 100 | 13 | 74 | 1.1 | 217820 | 11.34 | 1.06 |
| all1 | 4 | native | 1.1 [1.1, 1.3] | 19 | 1 | 12 | 1.4 | 216688 | 1.06 | 1.42 |
| all1 | 4 | refactored | 1.3 [1.2, 3.5] | 23 | 3 | 11 | 1.3 | 209673 | 1.29 | 1.30 |
| all1 | 4 | bridge_mutex | 1.9 [1.0, 4.1] | 18 | 7 | 178 | 1.1 | 208036 | 1.86 | 1.12 |
| all1 | 4 | mcs | 103.0 [103.0, 108.4] | 949 | 122 | 826 | 1.7 | 243263 | 103.04 | 1.68 |
| all1 | 4 | uscl | 3.4 [3.3, 3.8] | 11 | 16 | 384 | 1.4 | 205402 | 3.38 | 1.36 |
| all1 | 4 | fc | 16.3 [15.6, 17.3] | 187 | 19 | 160 | 1.4 | 219393 | 16.33 | 1.44 |
| all1 | 4 | fc_pq | 14.2 [13.7, 16.6] | 151 | 16 | 134 | 1.3 | 219820 | 14.23 | 1.28 |
| all1 | 8 | native | 3.9 [1.2, 4.0] | 28 | 24 | 587 | 1.4 | 204185 | 3.86 | 1.38 |
| all1 | 8 | refactored | 3.8 [1.3, 4.3] | 30 | 23 | 565 | 1.1 | 197698 | 3.79 | 1.12 |
| all1 | 8 | bridge_mutex | 3.9 [1.1, 4.0] | 24 | 29 | 745 | 1.4 | 197361 | 3.94 | 1.43 |
| all1 | 8 | mcs | 105.0 [103.2, 107.3] | 998 | 127 | 875 | 3.0 | 288082 | 104.97 | 2.96 |
| all1 | 8 | uscl | 3.8 [2.6, 4.0] | 11 | 21 | 581 | 1.4 | 203162 | 3.77 | 1.36 |
| all1 | 8 | fc | 23.3 [22.3, 23.6] | 326 | 28 | 299 | 1.6 | 235078 | 23.32 | 1.57 |
| all1 | 8 | fc_pq | 10.5 [10.4, 10.6] | 160 | 13 | 156 | 1.3 | 233068 | 10.46 | 1.31 |
| half1_half64 | 1 | native | 0.0 [0.0, 0.0] | 11 | 2 | 36 | 7.1 | 508337 | 0.00 | 0.22 |
| half1_half64 | 1 | refactored | 0.0 [0.0, 0.0] | 11 | 2 | 34 | 7.0 | 481168 | 0.00 | 0.22 |
| half1_half64 | 1 | bridge_mutex | 0.0 [0.0, 0.0] | 11 | 2 | 36 | 6.9 | 481944 | 0.00 | 0.21 |
| half1_half64 | 1 | mcs | 0.0 [0.0, 0.0] | 12 | 2 | 35 | 6.9 | 479844 | 0.00 | 0.21 |
| half1_half64 | 1 | uscl | 0.0 [0.0, 0.0] | 9 | 2 | 36 | 7.6 | 481344 | 0.00 | 0.23 |
| half1_half64 | 1 | fc | 0.0 [0.0, 0.0] | 12 | 2 | 38 | 9.8 | 481323 | 0.00 | 0.30 |
| half1_half64 | 1 | fc_pq | 0.0 [0.0, 0.0] | 11 | 2 | 34 | 6.6 | 481511 | 0.00 | 0.20 |
| half1_half64 | 2 | native | 1.0 [1.0, 1.0] | 19 | 2 | 12 | 0.8 | 251027 | 0.30 | 0.25 |
| half1_half64 | 2 | refactored | 1.2 [1.2, 1.3] | 21 | 2 | 36 | 8.2 | 404376 | 0.05 | 0.26 |
| half1_half64 | 2 | bridge_mutex | 1.0 [1.0, 1.0] | 18 | 1 | 6 | 0.9 | 245929 | 0.28 | 0.23 |
| half1_half64 | 2 | mcs | 109.4 [107.1, 111.9] | 659 | 126 | 738 | 4.3 | 494993 | 3.37 | 0.13 |
| half1_half64 | 2 | uscl | 4.8 [4.5, 5.0] | 22 | 6 | 57 | 5.1 | 380081 | 0.25 | 0.26 |
| half1_half64 | 2 | fc | 12.5 [11.7, 13.7] | 99 | 15 | 121 | 6.5 | 484382 | 0.38 | 0.20 |
| half1_half64 | 2 | fc_pq | 15.1 [12.7, 15.3] | 127 | 18 | 130 | 5.1 | 426208 | 0.60 | 0.21 |
| half1_half64 | 4 | native | 1.1 [1.1, 1.1] | 24 | 1 | 10 | 1.1 | 222279 | 1.06 | 1.08 |
| half1_half64 | 4 | refactored | 1.5 [1.3, 1.6] | 35 | 8 | 111 | 7.2 | 453525 | 0.04 | 0.21 |
| half1_half64 | 4 | bridge_mutex | 1.0 [1.0, 1.0] | 24 | 3 | 31 | 7.1 | 519149 | 0.03 | 0.21 |
| half1_half64 | 4 | mcs | 121.7 [120.7, 121.9] | 1021 | 148 | 855 | 5.2 | 537081 | 3.74 | 0.16 |
| half1_half64 | 4 | uscl | 8.0 [6.6, 8.3] | 19 | 38 | 861 | 3.8 | 352473 | 0.43 | 0.21 |
| half1_half64 | 4 | fc | 25.5 [25.2, 26.5] | 305 | 31 | 311 | 5.8 | 491258 | 0.79 | 0.18 |
| half1_half64 | 4 | fc_pq | 20.3 [19.2, 21.8] | 231 | 24 | 225 | 3.7 | 370470 | 1.19 | 0.20 |
| half1_half64 | 8 | native | 1.2 [1.1, 1.2] | 27 | 4 | 74 | 14.0 | 762422 | 0.02 | 0.24 |
| half1_half64 | 8 | refactored | 1.3 [1.3, 1.4] | 30 | 5 | 72 | 11.3 | 700895 | 0.02 | 0.23 |
| half1_half64 | 8 | bridge_mutex | 1.1 [1.1, 1.5] | 28 | 4 | 69 | 12.9 | 716402 | 0.02 | 0.20 |
| half1_half64 | 8 | mcs | 126.1 [124.5, 128.2] | 1051 | 159 | 906 | 3.5 | 616121 | 3.88 | 0.11 |
| half1_half64 | 8 | uscl | 7.8 [6.9, 8.0] | 18 | 47 | 1127 | 5.0 | 359571 | 0.41 | 0.26 |
| half1_half64 | 8 | fc | 31.2 [31.0, 32.0] | 494 | 38 | 509 | 4.4 | 513509 | 0.97 | 0.14 |
| half1_half64 | 8 | fc_pq | 12.1 [11.3, 12.4] | 238 | 15 | 261 | 5.1 | 440985 | 0.52 | 0.20 |

## Reading

Follow-up formal results (frequency-confounded at 4-8 clients) and the counter
cohort, None (the primary regime), unless stated otherwise.

- **Service fairness, `half1_half64`.** FC-PQ `service_jain` is 0.965 [0.965,
  0.966] at 2 clients, 1.000 [0.999, 1.000] at 4 and 0.963 [0.958, 0.971] at 8.
  - FC: 0.857 / 0.842 / 0.850; MCS: 0.872 / 0.879 / 0.872.
  - U-SCL: 1.000 at every count.
  - native: 0.500 / 0.662 / 0.136, i.e. one client dominates.
  - The 64-record half gets 0.59 / 0.51 / 0.59 of service under FC-PQ against
    0.69-0.72 under FC and MCS.
  - The Immediate control has the same order: FC-PQ 0.976 / 0.998 / 0.967,
    FC 0.88-0.90, MCS 0.87-0.88, U-SCL 1.000.
  - `half1_half8` is fair for all four locks (≥ 0.988), because 8-record
    bodies cost little more than 1-record ones.
- **Throughput, `half1_half64`** (median tx/s; records/s; frequency-confounded
  at 4-8 clients):

  | clients | FC-PQ | FC | MCS | U-SCL | native |
  |---|---|---|---|---|---|
  | 2 | 17,887; 438k | 16,833; 547k | 15,082; 490k | 20,602; 397k | 18,356; 472k |
  | 4 | 12,873; 246k | 10,326; 339k | 10,874; 353k | 5,361; 96k | 10,486; 288k |
  | 8 | 13,702; 318k | 10,984; 360k | 9,576; 311k | 4,902; 92k | 10,330; 219k |

  - FC-PQ/FC is 1.06 / 1.25 / 1.25 in tx/s and 0.80 / 0.73 / 0.88 in records/s.
  - FC-PQ/MCS records/s is 0.89 / 0.70 / 1.02; FC-PQ/U-SCL 1.10 / 2.57 / 3.47.
  - FC-PQ serves the 1-record half more, so it completes more transactions and
    fewer records than FC.
  - Per-body time (service utilization / tx/s) at 8 clients: FC-PQ 72 µs,
    FC 90, MCS 104, U-SCL 200; at 1 client every variant is ≈59 µs.
  - Of the two locks with `service_jain` ≥ 0.96, FC-PQ delivers 1.1-3.5× U-SCL's
    records/s. **perf-02:** at 2.2 GHz this is 1.05-1.21×. U-SCL's cores run at
    0.84-1.08 GHz at 4-8 clients (governor down-clocking of sleeping waiters).
- **`all1`** (frequency-confounded at 4-8): every lock is fair except the
  Mutex/Condvar controls. FC-PQ has the highest median tx/s at 4 and 8 clients
  (22,792 and 27,557 against FC 19,958 / 24,920, MCS 16,809 / 16,890 and U-SCL
  9,473 / 8,109). The ranges overlap FC's.
- **Fast path.** Hit rate 1.000 at 1 client, 0.181 (`all1`) and 0.242
  (`half1_half64`) at 2, and 0.000 at 4-8. At 1 client all seven variants are
  within 1-5 % (`all1` FC-PQ 33,136 vs FC 33,128 tx/s; `half1_half64` 17,146 vs
  17,096). At 2 clients FC-PQ/FC is 0.99 (`all1`). The fast path is therefore
  not visible in redb throughput. [INFERENCE] E0(b)'s ≈65 ns per request is
  ≈0.2 % of a 30-60 µs body.
- **Cache migration (counter cohort).** The question was whether per-transaction
  HITM/LLC misses grow with client count for mcs/uscl/native but stay flat for
  fc/fc_pq. Measured answer: **only MCS migrates heavily, and it steps up rather
  than grows. FC-PQ is flat and low; FC grows slowly. Native and U-SCL barely
  migrate, but for different reasons.**
  - **MCS:** HITM loads/tx go from 0.0 at 1 client to 94-109 at 2, then stay at
    103-126 through 8 clients. HITM-supplied lines are 659-1051/tx. Each FIFO
    hand-off moves the body's working set (B-tree, allocator and transaction
    state) to the next core.
  - **FC:** 8→16→23 (`all1`) and 13→25→31 (`half1_half64`); supplied lines
    63-494. It grows with clients but stays 4-9× below MCS.
  - **FC-PQ:** 10-15 (`all1`) and 12-20 (`half1_half64`) at 2-8 clients, flat.
    Supplied lines 100-238. [INFERENCE] The residue is the per-request payload
    and completion hand-off plus combiner changes. FC-PQ changes combiner less
    often than FC; the counters are consistent with this but do not show it.
  - **native, refactored, bridge_mutex: 1-4 HITM loads/tx at every count.** The
    Mutex/Condvar lets one client hold the write path for long stretches
    (`service_jain` down to 1/c), so there is little to migrate. Their all-miss
    L2 counts rise (up to 587-745/tx at 8 clients in `all1`) without HITM.
    [INFERENCE: the lines come from L3 after a sleeping core's private caches
    were flushed, as in a deep C-state.]
  - **U-SCL: HITM 2-8/tx**, because its 2 ms slice lets a holder run many
    transactions in a row. At 4-8 clients its L2 misses are the highest (384-1127/tx), again
    without HITM [INFERENCE: same flush mechanism, since U-SCL waiters sleep].
    Its user cycles per transaction (≈98-178k) are far below its ≈200 µs body
    time at 2.2-3.5 GHz. [INFERENCE] The rest is kernel time or low frequency on
    a woken core; user-only counters cannot tell which. **perf-02 settles
    it:** the clock. U-SCL runs at 0.84-1.08 GHz at 4-8 clients (ref_tsc and
    sampler agree).
  - **LLC misses are small for every lock** (≤ 3/tx in `all1`, ≤ 14/tx in
    `half1_half64`) and show no lock pattern. The working set fits the LLC; the
    migration cost is L2-to-L2 (HITM), not DRAM.
- **Does migration back the per-body gap?** Partly, for MCS against FC/FC-PQ.
  At 8 clients MCS has ≈10× FC-PQ's HITM loads/tx (126 vs 12) and ≈4× its
  supplied lines (1051 vs 238), with the same instructions per transaction
  within ±40 %. Its body is 104 µs vs FC-PQ's 72 µs. [INFERENCE] ≈110 extra HITM
  loads at ≈100-200 ns each is 11-22 µs of the ≈30 µs gap; the rest is
  unattributed and includes the frequency confound. It does **not** explain
  U-SCL's 200 µs body, which has few HITMs; its cost is outside user-mode
  cache migration (perf-02: it is the < 1.1 GHz clock).
- **CPU cost.** MCS/FC/FC-PQ spin: 2c CPU-s (16 at 8 clients). The
  Mutex/Condvar controls block (2.1-2.4 CPU-s). U-SCL yields and sleeps
  (≈3.9-4.0 CPU-s).

## Follow-up 2 (approved 2026-09-28): clock-normalised perf cohort (perf-02)

Details, tables and the overhead measurement:
[redb-perf-02-clock](./redb-perf-02-clock.md).

- **Implementation** (`run.py`):
  - adds `cpu_clk_unhalted.ref_tsc:u` (fixed counter 2; perf 7.2.5 maps the
    name `ref-cycles` to the programmable 0x013c) and records perf's running
    fraction per event;
  - samples each client CPU's `cpuinfo_avg_freq` and `/proc/stat` during perf
    cells;
  - reports `effective_ghz`, tx/s at the 2.2 GHz TSC rate and a mixed-clock
    flag.
- **Runs.** `.worktree/redb-perf-02`: 168 cells, 0 failed, 100 % running for
  every event. An interleaved 5-arm overhead run of 240 cells, 0 failed.
- **The `all1` two-level pattern collapses for FC and FC-PQ.**
  - Repeat spread at 4-8 clients: 17-38 % raw, 1-6 % at 2.2 GHz; cycles/tx
    0-6 %.
  - At 2.2 GHz every lock but MCS gives 18.9-21.0k tx/s from 1 to 8 clients.
  - MCS gives 0.78-0.84× FC-PQ and keeps a 16-18 % spread that does not come
    from the clock.
- **Ratios at 2.2 GHz.**
  - FC-PQ/FC: unchanged (`all1` 0.98-1.03).
  - FC-PQ/MCS: rises slightly (`all1` 1.20-1.28).
  - FC-PQ/U-SCL: falls from 2.1-2.6× to 0.92-0.99 (`all1` tx/s, 4-8 clients),
    and from 2.4-3.2× to 1.05-1.13 in `half1_half64` records/s. U-SCL's deficit
    is its cores' low clock.
- **Overhead.** Adding cycles + ref_tsc: paired ratio 0.997 raw and 1.002 at
  the sampler clock. perf stat as a whole costs ≈ 1 % (0.984 raw, 0.992 at the
  sampler clock). Both are within the noise.

## Follow-up 3 (approved 2026-09-29): fixed clock, S1 at 3.0 GHz

Details: [redb-perf-02-clock](./redb-perf-02-clock.md), section "Fixed clock,
S1 at 3.0 GHz".

- **Harness** (`run.py`):
  - `--power-setup S0|S1|S2` with `--fixed-ghz F`, recorded in the manifest
    (intel_pstate, every policy, cpuidle per run CPU).
  - A preflight that refuses a mismatching host, and a post-cell re-check.
  - An off-target flag outside F ± 2 %, and the reference clock F.
  - The clock sampler now runs in timed cells too.
  - `--sustain-probe`, `--check-power`, and `--variants`.
- **Runs** (host pinned by the operator: `performance`, min = max = 3.0 GHz on
  all 128 policies, turbo on, C6 on):
  - `.worktree/redb-perf-03-fixed3g`: 168 cells, 0 failed.
  - `.worktree/redb-formal-03-fixed3g`: 360 cells (native/mcs/uscl/fc/fc_pq),
    0 failed.
  - Cells off target: 10.7 % and 13.6 %, all 3-5 % below F.
- **Formal results at 3.0 GHz** (these supersede the S0 absolute numbers above
  for these five variants):
  - FC-PQ/U-SCL at 4/8 clients: `all1` tx/s 1.00/1.05, `half1_half64`
    records/s 0.92/1.27 (S0 raw 2.41/3.40 and 2.57/3.47).
  - U-SCL's contended body: 120-200 µs → 31-51 µs, with unchanged cycles/tx.
  - FC-PQ/FC and FC-PQ/MCS match S0's clock-normalised values (within 0.08;
    FC-PQ/MCS `half1_half64` c4-8 tx/s 1.30-1.36 against 1.42-1.54).
  - `service_jain` order unchanged: FC-PQ 0.941/0.995/0.946, FC 0.83-0.85,
    MCS 0.85-0.86, U-SCL 1.000.

## Open items

- S2 (C6 disabled on 16-23) perf cohort, to price C6 wake cost and the 3-5 %
  low clock of sleeping-waiter cells.
- Per-thread counters (e.g. `perf_event_open` inside the harness), to separate
  the combiner/holder from waiters.
- More repetitions at 4-8 clients.
- Optional: FC-PQ with and without the fast path in redb.
