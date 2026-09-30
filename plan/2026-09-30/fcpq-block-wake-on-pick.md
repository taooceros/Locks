# FC / FC-PQ blocking waiters: wake-on-pick (2026-09-30)

Status: approved 2026-09-30 (user instruction "spawn a few agents to test this
in a new worktree"). Workspace `.worktree/jj/fcpq-block`, on top of E0(a)
change `uwqronmm` (`spin_park` feature). Default builds unchanged.

## Goal

Show that FC-PQ's spinning cost is an implementation choice: a blocking
FC-PQ keeps its redb throughput and service-time fairness while cutting CPU
time toward u-SCL's. Follow the policy of TCLocks' blocking lock
(OSDI'23 §3.4; `rs3lab/TCLocks` `src/userspace/litl/src/kombmtx.c`,
`src/kernel/rcuht/locks/komb_mutex.c`), ShflLock-B (SOSP'19 §4.2.2) and
CFL's mutex (PPoPP'24 §3.3): one state word per waiter, the combiner wakes a
parked waiter when it *picks* the request, before executing it.

## Why the earlier park variants lost

- E0(a) `spin_park`: combiner wakes after the *next* delegate (deferred
  fence); a parked waiter pays full wake latency after a long wait. FC-PQ at
  8T/8 CPUs cs 20000 collapsed (0.033-0.050 vs 0.093 Mops/s spinning).
- `timed_park` (`zlqxmnyz`): 98% of parks end by a 50 us timeout (timer
  slack), so budget 0 lost 56-58% at cs 1000 with a CPU per thread.

## Changes

1. `ParkSlot` state word: `WAITING -> PARKED` (waiter CAS) and
   `* -> PICKED` (combiner swap; `futex_wake` iff old == `PARKED`). A
   waiter whose CAS fails spins on `complete`: its CS is already running.
   Same-word RMW on both sides; no Dekker fence, no deferral, no timeout.
   Reset to `WAITING` by the owner before publishing a request.
2. Wake point: FC when the scan reaches the node; FC-PQ when the request is
   popped from the heap, before its delegate runs. Optional FC-PQ lookahead
   `k` (wake the next heap top too), compile-time/env knob, default 0.
3. Unlock path (`parked_after_unlock`, combiner hand-off wake of a parked
   waiter when requests remain) keeps its liveness role; adapt to the new
   states.
4. Spin budget: existing `DLOCK_SPIN_BEFORE_PARK_US`; measured at 0 (block
   by default, TCLocks userspace), 5, 100.

Non-goals: barging/lock stealing, adaptive budget, accounting changes.

## Correctness

Loom model of waiter CAS vs combiner swap vs unlock wake (no lost wake, no
early result read, no stuck enrolled request); `dlock2_unit_test` suite and
`thread_churn` tests with the feature on and off.

## Evaluation

1. Microbenchmark, matrix as the timed-park comparison: fc, fc-pq-b-heap;
   8T/8, 8T/4, 32T/8 CPUs; cs 1000, 20000; variants spin (default),
   E0(a) spin_park 100, wake-on-pick budget 0/5/100, FC-PQ k=0/1.
   CPUs 16-23 under `flock ~/.cache/locks-experiments/measurement.lock`,
   fixed 3.0 GHz; interleaved, 3 runs. Mops/s, CPU-s, Jain, vcsw/s.
2. redb S1 cells (all1, half1_half64; 2/4/8 clients): FC-PQ spin vs
   wake-on-pick best config vs u-SCL. Needs the change ported onto the redb
   harness (`mnkkkmky`).

Acceptance (redb, budget 0): throughput within ~5% of spinning FC-PQ;
CPU-s near u-SCL (3.94 at c8; spinning FC-PQ 15.99); service Jain >= 0.94.

## Risks

- Futex wake latency (us) vs short CS: one early wake may not cover it;
  `k = 1` lookahead is the mitigation.
- One RMW per served request (measured +4-5% in timed-park, not a loss).
- Shared host: record load average; raw data only in `.worktree/` and
  `~/Locks-artifacts/`.

## Results (implementation)

Change `tyzntutr` ("block_park: TCLocks-style wake-on-pick waiters for FC
and FC-PQ") on `uwqronmm`. Feature `block_park` on `libdlock`, forwarded by
`dlock` and `upscaledb-bridge`; `spin_park,block_park` together is a
`compile_error!`. Protocol in `crates/libdlock/src/dlock2/README.md`
("Wake-on-pick blocking waiters") and the `park.rs` module docs.

### What was built

- `park.rs`: `block_park` `ParkSlot` with states `WAITING` / `PARKED` /
  `PICKED` (no `DONE`: `complete` already separates "picked" from
  "served"). `reset()` (owner, Relaxed, before the SeqCst
  `complete = false`); `park_or_lock()`: `parked += 1`,
  `CAS(WAITING -> PARKED)` (failure: `parked -= 1`, `Parked::Picked`),
  fence, re-check `complete` / `active`, last `try_lock`,
  `futex_wait(PARKED)`; on return `Complete` or `Picked`. `pick()`
  (combiner): `swap(PICKED)`; `parked -= 1` and `futex_wake` iff the old
  value was `PARKED`. `SpinBudget`, `SPIN_BEFORE_PARK` and
  `parked_after_unlock` are shared with `spin_park`; `WAKE_LOOKAHEAD`
  (`DLOCK_WAKE_LOOKAHEAD`, 0 or 1, default 0) added.
- FC: `pick()` when the scan reaches an active, incomplete node, before the
  delegate; the deferred `wake_if_parked` stays `spin_park`-only. FC-PQ:
  `pick()` right after the `!complete` check at pop; lookahead `pick()` on
  `job_queue.peek()` (reusing the prefetch peek) when its request is
  incomplete. Both `lock()` loops: `reset()` before publication; a
  `picked` flag makes `Picked` terminal (the waiter keeps the existing
  8-spin / `try_lock` round loop and never parks again for that request).
  Handshakes 2 (`release_combiner`) and 3 (enroll/retire CAS) unchanged,
  gated on either feature.
- Tests: `dlock2_unit_test::thread_churn` ported from the E0(b) churn test
  (`vsxnputn`, FC-PQ only there) and generalized to FC, FC-PQ heap and
  btree; `idle_holder_release` unchanged. Loom model
  `dlock2/park/loom_model.rs` (`RUSTFLAGS="--cfg loom"`,
  `[target.'cfg(loom)'.dev-dependencies] loom = "0.7"`; first loom use in
  the repo).

### Protocol deviations from the plan text, with reasons

- `PICKED` is terminal for a request: a waiter whose word is `PICKED`
  (CAS failed, or woken) spins with `try_lock` polling and never re-parks.
  Reason: allowing `PICKED -> PARKED` would need a second, post-completion
  wake (the E0(a) deferred fence again) to avoid a lost wake-up when the
  waiter re-parks between the pick and `complete = true`. Cost: a
  lookahead wake at the FC-PQ pass limit (H = 64) leaves that waiter
  spinning until it becomes combiner or the next pass serves it (rare;
  covered by the loom scenario `waiter_vs_lookahead_at_pass_limit`).
- The unlock hand-off keeps the E0(a) form (unlocker re-combines while
  `parked != 0`; the pass's `pick()` is what wakes the parked waiter)
  rather than a direct unlocker-side wake: one mechanism, already proven
  by `idle_holder_release`'s negative control. A waiter preempted between
  `parked += 1` and its CAS keeps the unlocker re-combining until it runs
  again; that window exists identically under `spin_park`.
- Lookahead is FC-PQ only (the plan's `k`); `k` is limited to 0/1 because
  `SequentialPriorityQueue` exposes one `peek()`.
- Budget 0 parks after one 8-spin round: `SpinBudget` arms its deadline
  on the first check (reused as-is, per the contract).

### Verification (CPUs 48-63, `taskset`; load average 5-25 on the shared host)

| Check | Result | Log |
|---|---|---|
| `cargo build --release -p libdlock` default / `spin_park` / `block_park` | ok / ok / ok | `.worktree/output/fcpq-block/tests/build-*.log` |
| `cargo test --release -p libdlock` default / `spin_park` / `block_park` | 155 passed, 3 ignored, 1 doc test each (152 + 3 `thread_churn`) | `tests/full-*.log` |
| `block_park` dlock2 tests, `cfg(test)` budget 5 us | 54 passed (incl. `idle_holder_release` x3, `thread_churn` x3) | `tests/block_park-budget5-dlock2.log` |
| `block_park` dlock2 tests, `DLOCK_SPIN_BEFORE_PARK_US=0` | 54 passed | `tests/block_park-budget0-dlock2.log` |
| `spin_park,block_park` | `compile_error!` as intended | (cargo check) |
| Parks happen (budget 0, `strace -f -e futex`, `idle_holder_release`) | 480 `FUTEX_WAIT(PARKED)`, 493 wakes, 336 hit a sleeper, 2 `EAGAIN` | `tests/strace-block_park-budget0-idle_holder.txt` |
| Machine code vs parent, default build (`objdump -d -C`, addresses, crate/LLVM hashes and constant-pool names normalized; one-time check) | 120 fc/fc_pq functions, 0 differing lines; 199 symbol sizes identical | `.worktree/output/fcpq-block/objdump/default/` |
| Same, `spin_park` build | 147 functions, 12 lines differ, all constant-pool data offsets (6 in `FCPQ::combine`, 6 in untouched `fc_sl` skiplist code); 0 opcode differences; 226 sizes identical | `objdump/spin_park/` |
| Loom model (see below) | see table | `tests/loom-block_park.log` |

Loom scenarios (owner with budget 0 vs. the others; every interleaving
must terminate with the right result, no `UnsafeCell` race, `parked == 0`
and the lock free):

| Scenario | Threads | Result |
|---|---|---|
| `waiter_vs_combiner_pick` (pick before/after park, missed enrollment, hand-off) | 2 | pass |
| `waiter_vs_idle_holder` (non-combining holder) | 2 | pass |
| `waiter_vs_retirement` (handshake 3) | 2 | pass |
| `waiter_vs_lookahead_at_pass_limit` (`PICKED` without service) | 2 | pass |
| `next_request_after_served` (stale `PICKED` / result from the previous request; starts from the post-service state) | 2 | pass |
| `waiter_vs_combiner_and_idle_holder` (holder inherits the re-combine obligation) | 3 | `#[ignore]`: loom starves the owner (see below) |

Modeling notes: the futex is a check-then-sleep under a loom mutex; every
model spin loop carries a `yield_now` (loom's spin hint) and a 300-iteration
guard, and the re-combine loop of `release_combiner` genuinely waits for a
preempted waiter's next step. A two-request, two-pass version did not finish
in 50 min of exhaustive exploration; `next_request_after_served` starts
from the served request's final state instead. The three-thread scenario
is kept but ignored: with a combiner and a holder both in the yield-looping
re-combine loop, loom 0.7 reaches a path where the owner sits between its
failed `PARKED` CAS and its `parked -= 1` undo (runnable) and is never
scheduled while the holder re-combines (guard message
`state=PICKED parked=1 complete=true`); the owner's pending step is one RMW
that the OS scheduler runs, and the two-thread scenarios cover the same
hand-off. Not a protocol finding; a loom scheduling limitation.

Build commands per variant: `.worktree/output/fcpq-block/BUILD-VARIANTS.md`.
Measurements: by the BenchRun / RedbRun batches (not in this section).


## Results (microbenchmark)

Machine: 2x Xeon Gold 6438M, kernel 6.17.7, shared. All runs under `flock ~/.cache/locks-experiments/measurement.lock` + `taskset` CPUs 16-23 (16-19 for 4 CPUs), cpufreq min=max=3.0 GHz checked before and after every run (none changed). Builds on CPUs 48-63, one `CARGO_TARGET_DIR` each under `.worktree/output/fcpq-block/targets/`. default / spin_park 100: parent `uwqronmm` export; block_park: Implementer commit d18ae3ed (lock code final; the working copy moved afterwards only for loom model/docs [INFERENCE: per Implementer's message]). `d-lock2 counter-proportional --non-cs 0`, 1 s warmup + 3 s measured, 3 interleaved runs per cell (198 runs, no hang or crash, smoke 32T/8 cs1000 first). Mops/s = total loop / cs / 3 s; CPU-s = user+sys of the 4 s process; Jain over per-thread ops; vcsw/s = voluntary ctx switches / 4 s. Runs were interrupted once by the tool timeout (143 runs done) and resumed from raw files, so the load average was not recorded for those runs (`runs.csv` load = "resumed"). Host load was high (other agents' tests and redb runs queued on the flock). Today's default spin is lower than the timed-park table (8T/8 cs1000 1.10 vs 1.46; cs20000 0.071 vs 0.093): host drift [INFERENCE], compare within this table only. Raw, scripts, `runs.csv`, `summary.csv`: `.worktree/output/fcpq-block/micro/`.

| Config | cs | Lock | Mode | Mops/s (runs) | CPU-s user+sys (sys) | Jain ops | vcsw/s |
|---|---|---|---|---|---|---|---|
| 8T / 8 CPUs | 1000 | fc | default (spin) | 1.19, 1.20, 1.20 | 32.0 (0.0) | 1.000 | 5 |
| 8T / 8 CPUs | 1000 | fc | spin_park 100 us | 1.16, 1.15, 1.18 | 32.0 (0.0) | 1.000 | 8 |
| 8T / 8 CPUs | 1000 | fc | block_park 0 | 0.46, 0.42, 0.42 | 11.6 (3.9) | 0.972 | 421k |
| 8T / 8 CPUs | 1000 | fc | block_park 5 us | 0.58, 0.77, 0.81 | 24.9 (2.1) | 0.992 | 430k |
| 8T / 8 CPUs | 1000 | fc | block_park 100 us | 1.10, 1.12, 1.16 | 32.0 (0.0) | 1.000 | 6 |
| 8T / 8 CPUs | 1000 | fc-pq-b-heap | default (spin) | 1.10, 1.10, 1.09 | 32.0 (0.0) | 1.000 | 6 |
| 8T / 8 CPUs | 1000 | fc-pq-b-heap | spin_park 100 us | 1.02, 1.03, 1.02 | 28.5 (0.1) | 1.000 | 7k |
| 8T / 8 CPUs | 1000 | fc-pq-b-heap | block_park 0 | 0.59, 0.59, 0.59 | 13.8 (3.4) | 0.998 | 444k |
| 8T / 8 CPUs | 1000 | fc-pq-b-heap | block_park 5 us | 0.85, 0.84, 0.83 | 20.3 (1.1) | 1.000 | 146k |
| 8T / 8 CPUs | 1000 | fc-pq-b-heap | block_park 100 us | 1.15, 1.17, 1.17 | 30.3 (0.0) | 1.000 | 4k |
| 8T / 8 CPUs | 1000 | fc-pq-b-heap | block_park 0, lookahead 1 | 0.61, 0.61, 0.61 | 16.9 (2.9) | 1.000 | 376k |
| 8T / 8 CPUs | 20000 | fc | default (spin) | 0.071, 0.072, 0.072 | 32.0 (0.0) | 1.000 | 4 |
| 8T / 8 CPUs | 20000 | fc | spin_park 100 us | 0.064, 0.064, 0.064 | 26.8 (0.3) | 1.000 | 34k |
| 8T / 8 CPUs | 20000 | fc | block_park 0 | 0.061, 0.064, 0.060 | 7.9 (0.5) | 0.933 | 62k |
| 8T / 8 CPUs | 20000 | fc | block_park 5 us | 0.064, 0.066, 0.065 | 9.0 (0.6) | 0.926 | 65k |
| 8T / 8 CPUs | 20000 | fc | block_park 100 us | 0.074, 0.074, 0.074 | 32.0 (0.0) | 1.000 | 663 |
| 8T / 8 CPUs | 20000 | fc-pq-b-heap | default (spin) | 0.070, 0.072, 0.071 | 32.0 (0.0) | 0.997 | 6 |
| 8T / 8 CPUs | 20000 | fc-pq-b-heap | spin_park 100 us | 0.058, 0.060, 0.056 | 18.2 (0.1) | 0.987 | 12k |
| 8T / 8 CPUs | 20000 | fc-pq-b-heap | block_park 0 | 0.060, 0.059, 0.059 | 7.9 (0.5) | 1.000 | 58k |
| 8T / 8 CPUs | 20000 | fc-pq-b-heap | block_park 5 us | 0.059, 0.060, 0.059 | 8.9 (0.5) | 1.000 | 58k |
| 8T / 8 CPUs | 20000 | fc-pq-b-heap | block_park 100 us | 0.070, 0.070, 0.070 | 18.9 (0.0) | 0.986 | 6k |
| 8T / 8 CPUs | 20000 | fc-pq-b-heap | block_park 0, lookahead 1 | 0.059, 0.061, 0.059 | 13.7 (0.4) | 1.000 | 49k |
| 8T / 4 CPUs | 1000 | fc | default (spin) | 0.61, 0.61, 0.61 | 16.0 (0.0) | 0.933 | 4 |
| 8T / 4 CPUs | 1000 | fc | spin_park 100 us | 0.59, 0.60, 0.59 | 15.9 (0.1) | 0.995 | 21k |
| 8T / 4 CPUs | 1000 | fc | block_park 0 | 0.12, 0.12, 0.09 | 5.9 (0.2) | 0.962 | 110k |
| 8T / 4 CPUs | 1000 | fc | block_park 5 us | 0.58, 0.57, 0.58 | 10.0 (0.0) | 0.997 | 1k |
| 8T / 4 CPUs | 1000 | fc | block_park 100 us | 0.58, 0.59, 0.56 | 10.3 (0.0) | 0.996 | 1k |
| 8T / 4 CPUs | 1000 | fc-pq-b-heap | default (spin) | 0.55, 0.55, 0.55 | 16.0 (0.0) | 0.953 | 4 |
| 8T / 4 CPUs | 1000 | fc-pq-b-heap | spin_park 100 us | 0.58, 0.58, 0.57 | 15.3 (0.1) | 0.999 | 16k |
| 8T / 4 CPUs | 1000 | fc-pq-b-heap | block_park 0 | 0.20, 0.15, 0.20 | 6.7 (0.6) | 0.999 | 130k |
| 8T / 4 CPUs | 1000 | fc-pq-b-heap | block_park 5 us | 0.57, 0.56, 0.55 | 10.4 (0.0) | 1.000 | 2k |
| 8T / 4 CPUs | 1000 | fc-pq-b-heap | block_park 100 us | 0.51, 0.52, 0.52 | 10.2 (0.0) | 0.998 | 1k |
| 8T / 4 CPUs | 1000 | fc-pq-b-heap | block_park 0, lookahead 1 | 0.14, 0.14, 0.13 | 7.4 (0.2) | 0.995 | 93k |
| 8T / 4 CPUs | 20000 | fc | default (spin) | 0.037, 0.036, 0.037 | 16.0 (0.0) | 0.514 | 4 |
| 8T / 4 CPUs | 20000 | fc | spin_park 100 us | 0.040, 0.039, 0.039 | 14.9 (0.2) | 0.958 | 21k |
| 8T / 4 CPUs | 20000 | fc | block_park 0 | 0.033, 0.033, 0.032 | 5.9 (0.3) | 0.909 | 33k |
| 8T / 4 CPUs | 20000 | fc | block_park 5 us | 0.031, 0.032, 0.032 | 6.4 (0.3) | 0.879 | 32k |
| 8T / 4 CPUs | 20000 | fc | block_park 100 us | 0.036, 0.037, 0.037 | 10.1 (0.0) | 0.989 | 1k |
| 8T / 4 CPUs | 20000 | fc-pq-b-heap | default (spin) | 0.037, 0.037, 0.037 | 16.0 (0.0) | 0.764 | 5 |
| 8T / 4 CPUs | 20000 | fc-pq-b-heap | spin_park 100 us | 0.042, 0.043, 0.043 | 12.9 (0.1) | 0.998 | 15k |
| 8T / 4 CPUs | 20000 | fc-pq-b-heap | block_park 0 | 0.036, 0.037, 0.035 | 6.3 (0.2) | 0.908 | 27k |
| 8T / 4 CPUs | 20000 | fc-pq-b-heap | block_park 5 us | 0.035, 0.037, 0.035 | 6.5 (0.3) | 0.891 | 28k |
| 8T / 4 CPUs | 20000 | fc-pq-b-heap | block_park 100 us | 0.037, 0.037, 0.037 | 10.2 (0.0) | 0.862 | 1k |
| 8T / 4 CPUs | 20000 | fc-pq-b-heap | block_park 0, lookahead 1 | 0.032, 0.031, 0.031 | 9.8 (0.2) | 0.998 | 27k |
| 32T / 8 CPUs | 1000 | fc | default (spin) | 0.31, 0.31, 0.31 | 32.1 (0.0) | 0.629 | 19 |
| 32T / 8 CPUs | 1000 | fc | spin_park 100 us | 0.42, 0.41, 0.41 | 30.4 (0.3) | 0.991 | 55k |
| 32T / 8 CPUs | 1000 | fc | block_park 0 | 0.25, 0.33, 0.18 | 8.9 (2.2) | 0.974 | 254k |
| 32T / 8 CPUs | 1000 | fc | block_park 5 us | 0.37, 0.23, 0.21 | 13.5 (1.9) | 0.982 | 267k |
| 32T / 8 CPUs | 1000 | fc | block_park 100 us | 0.43, 0.42, 0.42 | 16.0 (0.0) | 0.990 | 4k |
| 32T / 8 CPUs | 1000 | fc-pq-b-heap | default (spin) | 0.30, 0.30, 0.30 | 32.1 (0.0) | 0.328 | 22 |
| 32T / 8 CPUs | 1000 | fc-pq-b-heap | spin_park 100 us | 0.52, 0.49, 0.52 | 23.2 (0.2) | 0.998 | 24k |
| 32T / 8 CPUs | 1000 | fc-pq-b-heap | block_park 0 | 0.29, 0.26, 0.27 | 9.0 (2.5) | 1.000 | 254k |
| 32T / 8 CPUs | 1000 | fc-pq-b-heap | block_park 5 us | 0.23, 0.24, 0.23 | 11.6 (1.7) | 1.000 | 186k |
| 32T / 8 CPUs | 1000 | fc-pq-b-heap | block_park 100 us | 0.52, 0.52, 0.53 | 17.2 (0.0) | 0.997 | 5k |
| 32T / 8 CPUs | 1000 | fc-pq-b-heap | block_park 0, lookahead 1 | 0.23, 0.25, 0.24 | 12.0 (1.9) | 1.000 | 214k |
| 32T / 8 CPUs | 20000 | fc | default (spin) | 0.019, 0.018, 0.018 | 32.3 (0.0) | 0.427 | 22 |
| 32T / 8 CPUs | 20000 | fc | spin_park 100 us | 0.041, 0.041, 0.041 | 19.5 (0.4) | 0.992 | 41k |
| 32T / 8 CPUs | 20000 | fc | block_park 0 | 0.023, 0.021, 0.023 | 5.4 (0.2) | 0.931 | 22k |
| 32T / 8 CPUs | 20000 | fc | block_park 5 us | 0.023, 0.022, 0.022 | 5.7 (0.2) | 0.914 | 22k |
| 32T / 8 CPUs | 20000 | fc | block_park 100 us | 0.022, 0.021, 0.021 | 13.2 (0.2) | 0.966 | 22k |
| 32T / 8 CPUs | 20000 | fc-pq-b-heap | default (spin) | 0.019, 0.018, 0.019 | 32.1 (0.0) | 0.311 | 17 |
| 32T / 8 CPUs | 20000 | fc-pq-b-heap | spin_park 100 us | 0.036, 0.035, 0.036 | 16.6 (0.3) | 0.999 | 33k |
| 32T / 8 CPUs | 20000 | fc-pq-b-heap | block_park 0 | 0.031, 0.030, 0.032 | 6.1 (0.3) | 1.000 | 30k |
| 32T / 8 CPUs | 20000 | fc-pq-b-heap | block_park 5 us | 0.029, 0.029, 0.029 | 6.4 (0.2) | 1.000 | 28k |
| 32T / 8 CPUs | 20000 | fc-pq-b-heap | block_park 100 us | 0.024, 0.024, 0.024 | 12.8 (0.2) | 0.998 | 18k |
| 32T / 8 CPUs | 20000 | fc-pq-b-heap | block_park 0, lookahead 1 | 0.024, 0.025, 0.026 | 10.5 (0.2) | 1.000 | 24k |

### Reading

- **E0(a) FC-PQ cs 20000 collapse at 8T/8 CPUs: gone relative to same-day spin with budget 100 (0.093 nominal not reached by any variant).** FC-PQ spin 0.071, spin_park 100 0.058 (-18%, the collapse), block_park 100 0.070 (-1%). FC-PQ with budget 0/5 is 0.059-0.060 (-17%) at 8T/8, so those budgets do not collapse below spin_park but do not recover spin either, at 7.9-8.9 CPU-s instead of 32. At 32T/8 cs20000 block_park 100 (0.024) is better than spin (0.019) but worse than spin_park (0.036).
- **Budget 0 does not keep throughput with a CPU per thread:** 8T/8 cs1000 fc 0.43 vs 1.20 spin (-64%), fc-pq 0.59 vs 1.10 (-46%), at 11.6-13.8 CPU-s and ~430k vcsw/s. It is better than timed_park 0 (fc-pq 0.64 at the time vs 1.46, -56%) only modestly [INFERENCE: different host state]. At cs 20000 it costs -15 to -16% throughput at 8T/8 for 7.9 CPU-s. On 4 CPUs it is worse (0.12-0.20 at cs1000). Budget 5 is -23% (fc-pq) and -30..-40% (fc) at 8T/8 cs1000.
- **Lookahead 1 buys nothing in this matrix:** fc-pq budget 0: 8T/8 cs1000 0.61 vs 0.59 (+3%) with +3 CPU-s; 8T/4 cs1000 0.14 vs 0.18; 32T/8 cs1000 0.24 vs 0.27; cs20000 0.059 vs 0.059 (8T/8), 0.024 vs 0.031 (32T/8). Extra wakes burn CPU (13.7 vs 7.9 at cs20000 8T/8) without recovering throughput.
- block_park 100 at 8T/8 cs1000: fc 1.13 (-6% vs 1.20), fc-pq 1.16 (+6% vs 1.10), CPU-s 32.0 / 30.3. At 32T/8 cs1000 it matches spin_park (0.42 / 0.52) using 16.0 / 17.2 CPU-s instead of 30.4 / 23.2, Jain >= 0.99 (spin 0.33-0.63).

### Pick

No config qualifies against the 0.093 Mops/s cs 20000 spin reference (best block_park is 0.070). Closest is FC-PQ `block_park` budget 100 / lookahead 0; detail: FC-PQ `block_park`, budget 100 us, lookahead 0: the only block_park config within 5% of spin at 8T/8 cs1000, and no collapse relative to same-day spin (0.071). CPU saving is modest at 8T/8 (30.3 vs 32.0). Build: `taskset -c 48-63 env DLOCK_SPIN_BEFORE_PARK_US=100 DLOCK_WAKE_LOOKAHEAD=0 cargo build --release -p dlock --features block_park`. No config with budget < 100 qualifies, so the redb acceptance (budget 0, CPU-s near u-SCL) is not expected to hold for short critical sections; the redb cells should include budget 0 and 100 [INFERENCE].

Note on the pick criterion: the 0.093 Mops/s cs 20000 spin reference is from the earlier timed-park session. Same-day spin on this host was 0.071 (8T/8 FC-PQ) and `block_park` 100 measured 0.070, so against same-day spin there is no collapse. Against the nominal 0.093 no variant qualifies (spin itself is 0.071 today); `block_park` 100 is then the closest one. Validation: 66 cells x 3 runs, no FATAL/Traceback/clock change in the driver logs.

## Results (redb)

Workspace `.worktree/jj/fcpq-block-redb`, change `mzztyvkorznr` (E0(a) + block_park duplicated onto `mnkkkmky`, fast path kept; conflicts resolved with Implementer's confirmation). libdlock dlock2 tests with `block_park,fcpq_fast_path`: 63 passed, at the 5 us test budget and at `DLOCK_SPIN_BEFORE_PARK_US=0`. Harness: `integration/redb/build.py --block-park`, `run.py --clients`.

Setup: S1 at 3.0 GHz (`--check-power` passed), CPUs 16-23, node 0, exclusive flock, 2000 ms, 3 repeats, `none` durability (Immediate control also run, in raw data), patched tree 94132b5f (as the S1 reference). All FC-PQ builds have `fcpq_fast_path`. Reference rerun now: spin FC-PQ c8 CPU-s 15.99 vs U-SCL 3.95; Jain half1_half64 0.938/0.994/0.953 (archive 0.941/0.995/0.946). 108 + 54 + 54 cells, 0 failed; off-target clock cells (outside 3.0 +-2 %): ref 7/108, b100 2/54, b0 8/54 (kept). Loads before each batch 6-23. Medians of 3; voluntary context switches and park counts are not recorded by the harness [not measured]. Data: `~/Locks-artifacts/fcpq-block-redb/` (`ref-spin-uscl`, `block-b100`, `block-b0`).

| cohort | c | variant | tx/s | records/s | CPU-s | service Jain | tx vs spin |
|---|---|---|---|---|---|---|---|
| all1 | 2 | FC-PQ spin | 32,473 | 32,473 | 4.00 | 1.000 | 1.00 |
| all1 | 2 | FC-PQ block 100us | 32,207 | 32,207 | 4.00 | 1.000 | 0.99 |
| all1 | 2 | FC-PQ block 0 | 30,000 | 30,000 | 2.98 | 1.000 | 0.92 |
| all1 | 2 | U-SCL | 31,978 | 31,978 | 4.00 | 1.000 | 0.98 |
| all1 | 4 | FC-PQ spin | 30,991 | 30,991 | 8.00 | 1.000 | 1.00 |
| all1 | 4 | FC-PQ block 100us | 31,776 | 31,776 | 6.45 | 0.942 | 1.03 |
| all1 | 4 | FC-PQ block 0 | 31,275 | 31,275 | 3.95 | 1.000 | 1.01 |
| all1 | 4 | U-SCL | 31,201 | 31,201 | 3.99 | 1.000 | 1.01 |
| all1 | 8 | FC-PQ spin | 32,398 | 32,398 | 15.99 | 0.999 | 1.00 |
| all1 | 8 | FC-PQ block 100us | 33,312 | 33,312 | 8.30 | 0.941 | 1.03 |
| all1 | 8 | FC-PQ block 0 | 31,958 | 31,958 | 3.99 | 0.974 | 0.99 |
| all1 | 8 | U-SCL | 30,780 | 30,780 | 3.95 | 1.000 | 0.95 |
| half1_half64 | 2 | FC-PQ spin | 17,476 | 442,094 | 4.00 | 0.938 | 1.00 |
| half1_half64 | 2 | FC-PQ block 100us | 17,222 | 424,140 | 3.98 | 0.940 | 0.99 |
| half1_half64 | 2 | FC-PQ block 0 | 15,530 | 495,770 | 3.37 | 0.841 | 0.89 |
| half1_half64 | 2 | U-SCL | 20,242 | 358,300 | 3.99 | 1.000 | 1.16 |
| half1_half64 | 4 | FC-PQ spin | 19,049 | 364,762 | 8.00 | 0.994 | 1.00 |
| half1_half64 | 4 | FC-PQ block 100us | 13,420 | 544,163 | 6.55 | 0.702 | 0.70 |
| half1_half64 | 4 | FC-PQ block 0 | 15,784 | 487,245 | 3.98 | 0.836 | 0.83 |
| half1_half64 | 4 | U-SCL | 19,543 | 346,418 | 4.00 | 1.000 | 1.03 |
| half1_half64 | 8 | FC-PQ spin | 18,626 | 401,036 | 15.99 | 0.953 | 1.00 |
| half1_half64 | 8 | FC-PQ block 100us | 14,792 | 513,468 | 6.59 | 0.695 | 0.79 |
| half1_half64 | 8 | FC-PQ block 0 | 13,881 | 552,972 | 4.02 | 0.654 | 0.75 |
| half1_half64 | 8 | U-SCL | 19,293 | 334,986 | 3.95 | 1.000 | 1.04 |

Verdict at budget 0 (throughput within ~5 % of spin, CPU-s near U-SCL, service Jain >= 0.94): **all1 passes at c4 and c8** (tx/s 1.01, 0.99; CPU-s 3.95/3.99 = U-SCL; Jain 1.000/0.974); c2 misses throughput (0.92). **half1_half64 fails at every client count**: tx/s 0.89/0.83/0.75 of spin and Jain 0.841/0.836/0.654. Records/s rises (1.12/1.34/1.38x spin) because the 64-record half is served more (per-client service share at c8: 0.203 vs 0.047 for 1-record clients), the same skew that lowers service Jain; see `docs/reports/2026-09-30-blocking-fcpq.typ`. Budget 100 us keeps all1 throughput but saves little CPU (6.45 and 8.30 vs 8.00 and 15.99 CPU-s, not near U-SCL) and also fails half1_half64 at c4/c8 (0.70/0.79, Jain 0.70). Overall: **not accepted** on redb; CPU cost is fixed, fairness on mixed-length requests is not. [INFERENCE: wake-on-pick order departs from usage order when picks are woken late.]
