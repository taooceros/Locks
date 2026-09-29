# Combiner-thread fairness of delegation locks in a coroutine executor

Exploratory side study, separate from the thesis plan in the root `README.md`.
Plan: [`plan/2026-09-28/coro-delegation-study.md`](../../plan/2026-09-28/coro-delegation-study.md).
Crate: `crates/coro_delegation`. Build: `cargo build --release -p coro_delegation`.
Results: `crates/coro_delegation/results/` (JSON, one file per run).
Findings: `crates/coro_delegation/FINDINGS.md` (dated entries, newest first).

Results policy (a deliberate exception to the repo-wide rule that raw results
live under ignored `.worktree/`): the final result JSONs (950 files, 19 MB
apparent size) stay Git-tracked because they are the evidence behind every
number in `FINDINGS.md`, and `scripts/summarize.py` /
`scripts/summarize_tokio.py` regenerate its tables from them. Superseded runs
are not kept as loose JSON; each superseded set is one small `results/*.tar.zst`
archive (0.25–0.40 MB each) cited from `FINDINGS.md`. New exploratory runs
should go to `.worktree/`; only a set that backs a recorded finding is copied in.

## Question

In a cooperatively scheduled executor (coroutines/futures on a work-stealing
thread pool), a delegation lock turns one executor thread into the combiner
for as long as contention lasts (CES, König et al. arXiv:2511.09194, does this
by inline-resuming the next waiter on the unlocking thread). Three fairness
effects follow that a service-time Jain index over lock clients cannot see:

- F1 **Combiner burden**: which worker threads pay the administration and the
  execution of other tasks' critical sections, and how unevenly.
- F2 **Bystander delay**: tasks that never touch the lock but sit on the
  combiner's worker see their schedule->poll latency inflated by the whole
  combining pass.
- F3 **Service fairness** among lock clients under 1:8 critical-section cost
  heterogeneity: FIFO (dispatch mutex, CES, FC) gives share ∝ rate × cost;
  usage-ordered selection (FC-PQ) is the candidate fix. Does the fix survive
  the executor, and what does it cost?

Hypotheses to falsify:

- H-A: Under sustained contention the combiner role concentrates on a few
  workers (burden Jain < 0.7 at 8 workers) for CES and FC-style locks.
- H-B: Bystander p99 latency on combiner workers exceeds the non-combiner
  workers' p99 by more than the length of one pass (H × mean CS).
- H-C: Usage-ordered selection yields client service Jain >= 0.95 under 1:8
  heterogeneity where FIFO variants sit <= 0.90, at >= 0.9× the FIFO
  variant's throughput.
- H-D: Combiner-fairness mitigations (time-bounded pass; combiner rotation
  per pass; charging combining cycles as usage credit; electing the
  highest-usage client as combiner) restore burden Jain >= 0.9 and bystander
  p99 within 2× of non-combiner workers without breaking H-C.

## Lock variants (`src/locks/`)

All implement `lock::DelegationLock` / `lock::LockClient` (`src/lock.rs`).

| id | file | policy | placement |
|---|---|---|---|
| `dispatch` | `dispatch.rs` | FIFO mutex, tokio-style: unlock wakes next waiter via its Waker; owner continues | scheduler decides |
| `ces` | `ces.rs` | FIFO mutex; unlock suspends owner (`schedule_remote`) and inline-resumes next waiter (`schedule_inline`) | combiner thread = unlocking thread |
| `fc` | `fc.rs` | closure delegation, FIFO over publication order; combiner = task that wins `try_lock`; pass bound H=64 | closures run on combiner's thread; waiters woken via Waker |
| `fcpq` | `fc_pq.rs` | as `fc` but usage-ordered (binary heap keyed by charged cycles; newcomer usage = running mean; starvation clamp after 8 passes), mirrors `crates/libdlock/src/dlock2/fc_pq/lock.rs` in the main repo | same |
| `fcpq-*` | `fc_pq.rs` feature knobs | mitigations for H-D, each independently switchable via constructor options: `pass_budget_cycles`, `rotate_combiner`, `credit_combining`, `elect_max_usage` | same |

## Executor contract (`src/executor.rs`)

- `Executor::new(workers)`: `workers` OS threads, each pinned to a distinct
  physical core (`core_affinity`), each owning a `crossbeam_deque::Worker`
  local queue plus a shared `Injector`; idle workers park (`parking`).
- `Executor::spawn(fut) -> async_task::Task<R>`; schedule callback pushes to
  the current worker's local queue when on a worker, else to the injector.
- `executor::worker_id() -> Option<usize>`.
- `executor::schedule_inline(runnable)`: run on this worker before anything
  else, right after the current poll returns (deterministic LIFO slot).
  Worker-only.
- `executor::schedule_remote(runnable)`: push to the injector and unpark one
  other worker.
- `Executor::block_on(fut)`: drive `fut` on the calling (non-worker) thread.
- `Executor::shutdown() -> Vec<WorkerStats>`.
- Per-worker stats (`src/stats.rs`): cycles polling by `TaskKind`
  {Client, Bystander}, poll counts, bystander schedule->poll latency samples
  (fixed-capacity reservoir or log-linear histogram), combining cycles
  reported by locks through `stats::record_combining(worker, cycles)`.

## Workload (`src/workload.rs`, `src/bin/coro_bench.rs`)

- Shared structure: `BTreeMap<u64, u64>`; CS = `insert(random key)` plus
  `spin_cycles(class_cost)` inside the closure. Heavy class cost = 8× light.
- Client tasks: `n_clients` (default 64), half light, half heavy; each loops
  `run(cs).await` then `spin_cycles(parallel_work)` (default 4× light CS).
- Bystander tasks: `n_bystanders` (default = workers); each loops
  `spin_cycles(bystander_work)` then `yield_now().await`; the executor
  records schedule->poll latency per poll.
- Duration: fixed wall time (default 2 s) after 200 ms warm-up.
- CLI: `coro-bench --lock <id> --workers 8 --clients 64 --heavy-ratio 8
  --duration-ms 2000 --out results/<name>.json`.
- Output JSON: throughput (ops/s), per-client {ops, service_cycles,
  combining_cycles}, service Jain (over service_cycles), per-class
  run-latency p50/p99, per-worker {combining_cycles, bystander p50/p99,
  client poll cycles}, burden Jain (over per-worker combining_cycles).

Jain index: $J = (\sum x_i)^2 / (n \sum x_i^2)$.

## Method

1. Build executor + harness + `dispatch` + `ces`.
2. Build `fc` + `fcpq` against `src/lock.rs` only (executor-agnostic).
3. Run the matrix {lock} × {workers 4, 8} × {heavy-ratio 1, 8}, 3 repeats;
   record H-A..H-C verdicts in `FINDINGS.md`.
4. Implement mitigations; rerun; record H-D.
5. Each finding: what was measured, numbers, verdict, next question.

Rules: no per-request heap allocation on the steady-state path; `unsafe`
needs a `// SAFETY:` comment; run every benchmark with `--release`; never
run more workers than physical cores (`nproc` / 2 if SMT).
