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

All implement `lock::DelegationLock` / `lock::LockClient` (`src/lock.rs`),
except the coroutine-style `co-*` locks, which implement `lock::CoLock` /
`lock::AsyncMutex` (see "API assumption" below).

| id | file | policy | placement |
|---|---|---|---|
| `dispatch` | `dispatch.rs` | FIFO mutex, tokio-style: unlock wakes next waiter via its Waker; owner continues | scheduler decides |
| `ces` | `ces.rs` | FIFO mutex; unlock suspends owner (`schedule_remote`) and inline-resumes next waiter (`schedule_inline`) | combiner thread = unlocking thread |
| `fc` | `fc.rs` | closure delegation, FIFO over publication order; combiner = task that wins `try_lock`; pass bound H=64 | closures run on combiner's thread; waiters woken via Waker |
| `fcpq` | `fc_pq.rs` | as `fc` but usage-ordered (binary heap keyed by charged cycles; newcomer usage = running mean; starvation clamp after 8 passes), mirrors `crates/libdlock/src/dlock2/fc_pq/lock.rs` in the main repo | same |
| `fcpq-*` | `fc_pq.rs` feature knobs | mitigations for H-D, each independently switchable via constructor options: `pass_budget_cycles`, `rotate_combiner`, `credit_combining`, `elect_max_usage`; H-C knobs `starvation_clamp` (passes, 0 = off; label `-c<N>`) and `newcomer_init` (`mean`/`zero`/`min`/`median`; label `-n<init>`); `record_waits` (`--fcpq-wait-stats`, queue-wait histogram in the JSON) | same |
| `actor` | `actor.rs` | closure delegation to one dedicated server task per lock (spawned lazily by the first request): drain the request stack, serve ≤ H=64 in FIFO order, yield; park when empty | server woken, yields and wakes clients with default placement |
| `actor-inline` | `actor.rs` | as `actor` | server woken into the publisher's run-next slot (`wake_inline`), yields `home`, clients woken `remote` |
| `dispatch-pq` | `dispatch_pq.rs` | usage-ordered mutex *without* delegation (u-SCL style): the owner runs its own closure (charged `cycles()`); waiters sit in FC-PQ's heap (`fc_pq::UsageQueue`, shared code: same accounting, newcomer init, clamp); release hands ownership to the min-usage waiter; clamp counted in handoffs (default 16, label `-c<N>`); uncontended acquire = one CAS; `--handoff-stats` records the handoff cycle breakdown | handoff wake per `--wake-placement` (`default` = `dispatch`'s local-queue wake) |
| `cfl` | `cfl.rs` | usage-fair queue lock *without* delegation, async CFL/ShflLock: lock word (`LOCKED`, `NO_STEAL`) + MCS tail-swap list of lock-owned per-client nodes; the queue head (next owner) scans the waiters behind it while the owner runs and splices the min-usage one (enqueue-time charged cycles, FC-PQ accounting via `UsageNode`) behind itself; the tail is never moved, so waiters stay in arrival order and the clamp (handoffs since enqueue > N, default 256, label `-c<N>`) checks only the oldest; the head is woken when it becomes head (pre-wake), spins ≤ `--head-spin-cycles` (default 8000, label `-spin<N>`) on the word, then parks (SeqCst Dekker with the releaser). Knobs: `--cfl-shuffle off` (MCS FIFO, `-noshfl`), `--cfl-prewake off` (wake at release, `-nopre`), `--cfl-scan overlap` (scan only while held, `-ovl`); `--handoff-stats` → JSON `cfl` | own default `home`; `--wake-placement remote` → `-remote` |
| `co-fifo` | `co_mutex.rs` | coroutine-style FIFO mutex (`lock().await` / `unlock().await`, CS = the task's own code): `dispatch-pq`'s lock word (fast-path CAS), intrusive FIFO of waiter nodes embedded in the pinned `lock()` future; `unlock().await` hands ownership to the head, wakes it inline and steps the releaser aside once (`--co-step-aside remote` = injector, `home` = back of this worker's queue, `none` = no suspension, as `Drop`); chain bound K (`--ces-chain-bound`, default 64, 0 = off) with `ces`'s break (`--wake-placement`); `Drop` of the guard = synchronous release (counted `sync_drops`) | grantee runs next on the releaser's worker |
| `co-pq` | `co_mutex.rs` | as `co-fifo`, successor = `fc_pq::UsageQueue` minimum (usage = cycles from `lock()` returning to `unlock()`, charged to the handle; clamp in handoffs, default 256, label `-c<N>`) | same |

## API assumption (2026-09-30, user decision)

Locks are used in coroutine style, and this is binding for the study from
2026-09-30 on:

```rust
let mut g = h.lock().await;   // h: the task's handle to the mutex
/* critical section: the task's own continuation on *g, may .await */
g.unlock().await;             // explicit async release
```

The critical section is the task's own code, not a shipped closure.
`unlock().await` is where the lock may suspend the releaser once (step
aside) so the next owner runs first; `drop(g)` must still release correctly
(synchronously), but it is not the measured path. The API sits on a per-task
handle (`CoLock::handle`) because usage-ordered locks charge each critical
section to a client and the executor has no task-local storage; waiter nodes
live in the pinned `lock()` future. The closure-based `DelegationLock`
variants stay as references. Per-op lock cost `o = (T − CS) / ops` (window
minus critical-section cycles, per op) and the burden definition of
`stats::record_foreign_cs` (critical-section cycles run on a worker for
requests made on another worker; `ces` and `co-*`) are reported for every
run (`o_cycles_per_op`, `burden_foreign_jain`); `--parallel-mode
{spin,yield}` applies to every lock (no `sleep`: the executor has no timer).

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
- `executor::spawn_here(kind, fut) -> Option<Task<R>>`: spawn from inside a
  task onto the current worker's executor (first schedule as for any wake,
  default = local queue); `None` off-worker. Used by `actor` to start its
  server task.
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
