# redb: delegation locks inside the write path

This experiment patches redb 3.1.0 so that a lock (Mutex, MCS, U-SCL, FC,
FC-PQ) **is** redb's write-serialisation mechanism, following the UpScaleDB
pattern in [../upscaledb/](../upscaledb/README.md): one extracted write body,
`upstream`/`upstream_gate`/`std_mutex` controls, a pinned source with numbered patches, a
real-database correctness gate and the existing fresh-process runner. Fairness
is measured as **service-time share** (Jain over per-client time inside the
write body), next to the transaction-count Jain.

It replaces the earlier harness that wrapped whole transactions of unmodified
redb in external locks. In that design redb's own single-writer lock still
decided admission and was never contended, so it did not test delegation inside
the database. That harness and its claims are gone; its historical results and
worktrees (for example `experiment-redb-fair-transactions`) are untouched.

## Design decision recorded before implementation: admission-only replacement

**Question.** Should we also measure *admission-only* replacement, i.e. put the
lock inside `TransactionTracker::start_write_transaction`/`end_write_transaction`
and keep the public `Database::begin_write` API unchanged?

**Decision: not measured in this cutover; no admission-only variant is built.**

- It can only host lock-style backends (Mutex, MCS). FC and FC-PQ cannot hold
  ownership after returning to the caller: `begin_write` returns while the
  caller's transaction is still live, so a combiner would have to "own" a lock
  across arbitrary caller code. That is exactly the question under study, so an
  admission-only variant cannot compare FC-PQ with anything.
- For MCS, it would measure the substitution of redb's Mutex/Condvar hand-off
  by an MCS hand-off. The primary `mcs` variant already runs the identical
  closure body under MCS on the requesting thread; the code between begin and
  commit is the same in both designs, so the extra variant would mostly
  re-measure the same critical section while adding a second patch surface to
  `start_write_transaction`.
- If it is ever needed (e.g. to measure unchanged-API applications with arbitrary
  caller code inside the transaction), it must be a separately labelled variant
  (`admission_mcs`) with its own cohort, never mixed into the primary comparison.

## Application-logic boundary

Each submission is **one whole write transaction as a closure** (patched redb,
`redb::dlock_private`, not a public redb API):

```rust
let (txn_id, value) = gate.execute(|tx: &mut WriteTransaction| -> Result<R, E> {
    tx.set_durability(Durability::Immediate)?;
    let mut t = tx.open_table(ACCOUNTS)?;
    /* reads, inserts, updates, other tables ... */
    Ok(value)                    // Ok => commit, Err => abort
}, |call| bridge.submit(call))?;
```

`F: FnOnce(&mut WriteTransaction) -> Result<R, E> + Send`, `R: Send`,
`E: Send + From<CommitError>` (commit errors surface as `E`). Patched redb
contains one internal body, `write_body`: begin → closure → commit on `Ok`, or
abort on `Err`. The closure may call `set_durability`, `set_two_phase_commit`,
`set_quick_repair`, savepoint methods and open/delete tables; it cannot
`commit`/`abort` (both take the transaction by value). The error type
`DelegatedWriteError<E>` separates the closure's `E` (`Write`) from a failed
begin (`Begin`), a failed abort after `Err` (`AbortFailed`) and a call the
bridge never ran (`NotExecuted`). Upstream redb code performs the storage,
B-tree, allocator and commit work unchanged; only the writer admission differs:

- **upstream_gate**: `dlock_private::execute_upstream_gate` runs the body; its begin step
  is the unchanged public `begin_write`, i.e. redb's original tracker
  `Mutex<State>` + `live_write_transaction_available` Condvar, on the caller.
- **delegated** (`std_mutex`, `mcs`, `uscl`, `fc`, `fc_pq`, `fc_pq_hn`, `fc_pq_h8`): `DelegatedWriteGate::execute`
  hands the *same* body to the lock through a synchronous submit closure. Mutex,
  MCS and U-SCL run it on the requesting thread; FC and FC-PQ may run it on a
  combiner.
- **upstream** (upstream crates.io redb): the harness runs the same closure with
  the same begin → closure → commit/abort (and panic) sequence.

The harness workloads are closures shared by every variant (`src/writer.rs`):

- **Fixed insert** (`insert_body`, cohorts `all1`, `half1_half8`, `half1_half64`):
  1–64 new `u64 -> u64` records into one pre-existing table with one durability;
  an existing key aborts the whole transaction. The 1..=64 shape is checked on
  the caller before submission (no lock, no transaction ID). Identical semantics
  to the former fixed-insert body.
- **Transfer** (`transfer_body`, cohort `transfer`): a 1024-account table (1000
  each, created before the gate); each transaction reads two distinct accounts,
  writes the debit, then aborts if the source balance is below the amount
  (1–500), else writes the credit: 2 reads + 2 updates, or a rolled-back debit.

The critical section is therefore the whole transaction, **including commit I/O**
and whatever the closure does. Read transactions (`Database::begin_read`) **stay
outside every write lock and the bridge**; they register with redb's tracker
exactly as upstream.

### Closure contract

- **Borrowing.** The requester blocks until its closure has finished, so the
  closure may borrow the caller's stack (no `'static`). Type erasure is a thin
  context pointer plus a monomorphised trampoline (`DelegatedCall` /
  `RawDelegatedCall`); the context holds the closure (taken exactly once) and an
  outcome slot the executor fills before the lock returns. The higher-ranked
  `submit` bound keeps a `DelegatedCall` inside `submit`; `into_raw` (needed by
  locks with `'static` requests) is `unsafe` with that same obligation.
  `F`/`R`/`E: Send` are the whole justification for sending the call.
- **Panics.** A panic inside the closure is caught on the executing thread; the
  body then aborts the transaction **explicitly** (redb's `WriteTransaction::drop`
  skips the abort while the thread is panicking, which would leak the
  transaction's pages) and stores the payload; `execute` re-raises it on the
  **requester** with `resume_unwind`, after the lock has been released. A
  combiner never unwinds and the lock stays usable. On FC/FC-PQ the default panic
  hook prints the message on the combiner's thread. Any other panic in a
  delegated body (begin, commit, abort, the one-writer assertion, the bridge,
  the lock) aborts the process. `upstream_gate` and `upstream` follow the same
  catch → abort → resume sequence for closure panics; a redb-internal panic there
  unwinds on the caller as upstream.
- **Documented, not enforced.** No `begin_write` inside a closure (it waits on
  the gate forever, or on itself for `upstream_gate`/`upstream`, as a second upstream
  `begin_write` would). No nested submission: the bridge's thread-local depth
  guard refuses it and the inner call returns `NotExecuted` (also on a combiner).
  No waiting on other threads' progress (it stalls the lock, and on FC/FC-PQ the
  combiner and every requester it serves). Closures run by FC/FC-PQ run on the
  combiner's thread and see its thread-locals. `R: Send` and the closure lifetime
  keep table handles and the transaction inside the body. FC-PQ charges the whole
  closure to the requester's usage.

### Invariants on the delegated path

| redb rule | How it holds without the tracker's writer lock |
|---|---|
| One live writer | `DelegatedWriteGate::enter(&Database)` occupies the tracker's writer slot once (waiting on the unchanged Condvar, like any writer), so public `begin_write` callers wait until the gate is dropped; a second gate is refused. Per transaction, the body claims a delegated writer slot by atomic compare-exchange and **panics (→ process abort) if another writer is live**. |
| Transaction-ID order | Entering the gate copies the tracker's ID counter; the body allocates `last + 1` inside the delegation lock; leaving writes it back. Aborted and panicked transactions consume IDs exactly as upstream aborts do. |
| Reads | Untouched: readers take the last committed ID via the tracker's state mutex, as upstream. The commit code's reader/savepoint bookkeeping calls are unchanged. |
| Savepoints | Closures may create/restore/delete savepoints; allocation uses the tracker's state mutex (not the writer slot) as upstream. Savepoints taken before, during or after a delegated phase stay valid. |
| Abort | Every `Err` and every caught closure panic ends in `WriteTransaction::abort` on the executing thread (rollback of uncommitted pages, writer slot released), so the lock is always released normally. |

Per transaction, the delegated path never takes the tracker's `Mutex<State>` or
waits on its Condvar for admission (it cannot: the gate already holds that slot).
Transaction and guard objects are created and dropped inside the body on the
executing thread; only the closure, its value/error and a panic payload cross
threads.

## What is patched

`build.py` verifies the crates.io archive `redb-3.1.0.crate` against its pinned
SHA-256 (the same checksum `Cargo.lock` locks for the upstream build), extracts it,
applies these patches in order and places the result at the fixed Cargo path
dependency `.worktree/redb-src/redb-3.1.0` (generated, ignored):

| Patch | Files | Change |
|---|---|---|
| `patches/0001-delegated-writer-admission.patch` | `transaction_tracker.rs`, `db.rs`, `transactions.rs` | Delegated mode (enter/exit), delegated start/end writer slot with assertion, delegated `TransactionGuard`, `Database::begin_delegated_write` (a copy of `begin_write` with delegated admission), transaction-ID accessor |
| `patches/0002-closure-write-body.patch` | new `dlock_private.rs`, `lib.rs`, `Cargo.toml` | The closure body (`write_body`), `execute_upstream_gate`, `DelegatedWriteGate`, `DelegatedCall`/`RawDelegatedCall`, `DelegatedWriteError`, the body's service-time span (`Served`, returned by `execute_upstream_gate_served` and `DelegatedWriteGate::execute_served`; feature `dlock_service_time`), and the `dlock_test_hooks` feature (correctness probes only: occupancy, pause before commit, a panic outside the closure, the executed service total) |

To change a patch, apply the pinned crate plus the patches in a scratch git tree
under `.worktree/`, edit there, and re-export each patch with `git diff`
against the upstream commit (0001: `db.rs`, `transaction_tracker.rs`,
`transactions.rs`; 0002: `Cargo.toml`, `dlock_private.rs`, `lib.rs`); keep the
numbering. Format with `rustfmt --edition 2024`, as upstream.

`build.json` records the crate SHA-256 and origin, upstream and patched tree
hashes, each patch hash, harness/libdlock source hashes, `rustc -vV`, git state,
each binary's SHA-256 and its features three ways: the harness features passed to
Cargo, the libdlock/redb features Cargo resolved (`cargo metadata`), and the
features the binary reports (`--build-info`); the build stops if the last two
disagree with the request. An existing patched tree that differs from a fresh
application stops the build.

## Variants

| Variant | Binary | Write serialisation |
|---|---|---|
| `upstream` | `redb-upstream` | Upstream crates.io redb, untouched; the harness runs the same closure sequence on public `begin_write`/`commit` |
| `upstream_gate` | `redb-patched` | Shared closure body under redb's original Mutex/Condvar writer gate (`begin_write`) |
| `std_mutex` | `redb-patched` | Shared body via the bridge, `std::sync::Mutex` (redb's primitive type; the tracker's own mutex cannot be borrowed because commit re-enters it) |
| `mcs` | `redb-patched` | Shared body via the bridge, libdlock MCS, on the requester |
| `uscl` | `redb-patched` | Shared body via the bridge, U-SCL (`c/u-scl` fairlock in bridge mode, equal weights), on the requester |
| `fc` | `redb-patched` | Shared body via the bridge, libdlock FC, possibly on a combiner |
| `fc_pq` | `redb-patched` | Shared body via the bridge, libdlock FC-PQ built with the E0(b) `fcpq_fast_path`, possibly on a combiner |
| `fc_pq_hn` | `redb-patched` | As `fc_pq` with the per-pass pop cap H = `active` (entries in the priority queue at pass start); opt-in, see below |
| `fc_pq_h8` | `redb-patched` | As `fc_pq` with H = 8; opt-in, see below |

### FC-PQ pass-length variants (opt-in)

`fc_pq` keeps FC-PQ's original cap of 64 pops per combining pass (`PassCap::DEFAULT`,
so every earlier result stays valid). `fc_pq_hn` and `fc_pq_h8` build the same lock with
`FCPQ::with_pass_cap(…, PassCap::Active | PassCap::Fixed(8))`; the cap is a runtime field
read once per pass, so one binary hosts all three. They are **not** in the default
`--variants` set (the default matrix stays the seven original variants); name them at
preparation, for example `--variants fc,fc_pq,fc_pq_hn,fc_pq_h8,upstream_gate`. Plan and
result: [plan](../../plan/2026-09-29/fcpq-pass-length-ablation.md),
[evidence](../../docs/evidence/fcpq-pass-length-2026-09-29/RESULTS.md).

Extra per-cell data (present for every variant that has the quantity):

- `response_ns_hdr` per worker: response times of the windowed requests with 8
  sub-buckets per octave (12.5 % resolution), analysed as per-worker `worker_p50_ms` /
  `worker_p99_ms` (bucket upper bounds) plus the merged and short/long-group values;
- `served_for_others` and `executed_bodies` per worker (FC and FC-PQ variants): the bodies that
  worker's thread ran, over its whole run, for other requesters only and in total (its own
  requests included) - the requests served while it was the combiner. The analysis gives
  `max_worker_executed_share` (combiner concentration), `served_remote_fraction` and
  `own_local_fraction` (requests that ran on their own requester's thread); the trial
  fails if the workers' executed bodies differ from the committed transactions of an insert
  cohort. Two thread-local increments per body, always on;
- `pass_stats` (FC and FC-PQ, **stats binary only**): passes, empty passes, bodies,
  combiner changes (the combiner differs from the previous pass's; `_nonempty` compares only
  passes that ran a body), cap violations and a bodies-per-pass histogram. The analysis
  reports `bodies_per_pass` (ratio of totals), `bodies_weighted_pass_length` and
  `combiner_changes_per_1k_bodies`. A fast-path request counts as a one-body pass.

The counters live in the separate `redb-patched_stats` binary (harness and libdlock feature
`combiner_pass_stat`), because they add work inside the combining pass. Preparing with
`run.py --prepare-only --stats-binary` maps the delegated variants onto it, and
`build.py --uninstrumented` does not build it. `--prepare-only` also accepts `--clients`,
`--cohorts` and `--durabilities` to restrict the
matrix (recorded in the manifest as `subset`); the defaults are unchanged.

`redb-test_hooks` (patched redb with `dlock_test_hooks`) is used only by the
correctness gate and is never timed. Every binary is built with the harness
features `service_time` and, for the patched ones, `fcpq_fast_path` and
`fcpq_fast_path_stat` (forwarded to libdlock by `Cargo.toml`).

| File | Purpose |
|---|---|
| `patches/` | Numbered patches against redb 3.1.0 |
| `build.py` | Pinned source preparation, three `--locked` builds, `build.json` provenance |
| `correctness.py` | Real-database gate for every variant |
| `run.py` | Prepare/smoke/formal fresh-process runner and offline analysis |
| `src/writer.rs` | Variants, workload closures and the bridge (`Bridge`, the only code that touches libdlock) |
| `src/main.rs`, `src/selftest.rs` | Timed workload and self-test cases |

U-SCL follows `crates/upscaledb-bridge`: the fairlock lives at a stable boxed
address, is initialised in bridge mode, every thread registers once with weight
1024, and the lock is destroyed only after every submitting thread has been
joined. Its waiting is upstream U-SCL's own (futex queue hand-off, `nanosleep`
while banned, `sched_yield` after 20 spins), and its slice is
`FAIRLOCK_GRANULARITY` (2 × 2400 × 1000 cycles, ≈ 2.2 ms at this host's 2.2 GHz
TSC). MCS, FC and FC-PQ waiters spin; `upstream`, `upstream_gate` and `std_mutex`
block on redb's Mutex/Condvar or `std::sync::Mutex` (futex).

## Service time and fairness metrics

The closure body `write_body` reads the TSC with `rdtscp` once `begin` has
returned (admission is complete) and again once commit or abort has returned,
**on whichever thread runs it**: the combiner for FC/FC-PQ, the requester for
upstream_gate, std_mutex, MCS and U-SCL. The difference travels back with the outcome
(`Served::service_tsc`, from `execute_upstream_gate_served`/`execute_served`;
`execute`/`execute_upstream_gate` are unchanged) to the requester, which is charged.
`upstream` measures the identical span around the same public-API calls in the
harness. The span therefore excludes every admission wait, the bridge and lock
code, and `WriteTransaction` construction inside `begin`, for all seven variants
alike; it includes the closure and commit I/O. A panicked closure is not charged
(the requester receives the payload), and the harness charges committed
transactions only: an aborted transfer held the lock too, but its service is not
counted (see Known limits).

Per cell the analysis reports:

- `service_jain`: Jain over per-client service ticks credited to the 2 s window
  (the same requests as the transaction counts);
- `worker_service_share` per client and `long_service_share` (the K-record half's
  share; 0.5 is equal service);
- `tx_jain` (transaction counts), `service_utilization` (Σ service / window; ≤ 1
  because bodies are serialised);
- `fast_path_hit_rate` for `fc_pq`: `FCPQ::get_fast_path_hits` summed over the
  clients' threads / their committed transactions (`fcpq_fast_path_stat`,
  E0(b)'s +0.2 ns counter). In the insert cohorts every request commits; for
  `transfer` the hits also count aborted requests.

**Always on, no separate profile build.** The primary binaries carry the
instrumentation. `build.py --uninstrumented` builds binaries without
`service_time` and `fcpq_fast_path_stat` (fast path kept) for the overhead check
only; `run.py` and the gate refuse them. Measured on the redb-internal
fixed-body non-LTO build, one smoke cell (`fc_pq`, `all1`, None, 8 clients, CPUs
16-23, 8 ABAB pairs of 2 s runs, 2026-09-28): instrumented 33,464 tx/s [29,836,
34,091] vs uninstrumented 33,691 [33,377, 34,308]; paired ratio median 0.988
[0.894, 0.995], lower in 8/8 pairs. The ≈1 % cost is below the smoke's repeat
spread but consistent; two `rdtscp` per ≈30 µs request predict ≈0.1 %, so the
rest is probably code layout [INFERENCE]. Every variant carries the same two
reads per request.

## Clients and cohorts

A cell with c clients runs c saturated requesters, pinned one per CPU on the
first c prepared CPUs, and the trial process is restricted to exactly those CPUs
(`taskset`), so there are never more client threads than CPUs (no
oversubscription). The sweep is c ∈ {1, 2, 4, 8} (only counts ≤ the prepared CPU
set run; the trial binary accepts 1..=8 CPUs). The half cohorts need 1 client or
an even count; `all1` and `transfer` run at any client count.

| Cohort | 1 client | 2 clients | 4 clients | 8 clients |
|---|---|---|---|---|
| `all1` | 1 × 1 record | 2 × 1 | 4 × 1 | 8 × 1 |
| `half1_half8` | one client alternating 1, 8, 1, 8, … | 1 + 8 | 2 × 1 + 2 × 8 | 4 × 1 + 4 × 8 |
| `half1_half64` | one client alternating 1, 64, 1, 64, … | 1 + 64 | 2 × 1 + 2 × 64 | 4 × 1 + 4 × 64 |
| `transfer` | 1 × transfer | 2 × transfer | 4 × transfer | 8 × transfer |

Half the requests carry 1 record and half K in every half cohort. At 1 client
there is no contention and every per-client Jain index is 1 by definition; the
cell is the uncontended reference for the same request mix and shows FC-PQ's fast
path. `long_service_share` and the short/long splits exist only when every client
has one request size (not for the 1-client mixed cell or `transfer`).

## Durability regimes

**`None` is the primary regime**: the critical section is the B-tree and commit
work without fsync, so lock algorithms and serving order are visible.
**`Immediate` is a control**: every transaction holds the lock across an fsync,
which dominates and dilutes lock differences. The runner, analysis tables and
plots list `None` first.

## Cache-counter profile cohort

A separate profile cohort (`run.py --perf`) runs every cell under `perf stat` for
the whole process: None; `all1` and `half1_half64`; 1, 2, 4 and 8 clients; seven
variants; three repetitions = 168 cells. Timed cells (`--smoke`, `--run`) never
run perf. The same binaries are used. perf starts with counters disabled
(`--delay=-1 --control fifo:…`); with `REDB_PERF_CONTROL` set, the harness enables
them once every client is ready and disables them after all are joined, so
database creation, verification and close/reopen are not counted. Counters are
user mode only (`perf_event_paranoid` = 2 here) and process totals: perf stat
cannot attribute a launched command's threads separately. The combiner's work is
therefore not separated from the waiters' spinning.

| Name | Event | Meaning |
|---|---|---|
| `hitm_loads` | `mem_load_l3_hit_retired.xsnp_fwd` | retired loads served by a HitM snoop from another core on the socket |
| `hitm_supplied` | `core_snoop_response.i_fwd_m` | modified lines this core gave up to an invalidating snoop (loads and RFOs) |
| `l2_miss_loads` | `mem_load_retired.l2_miss` | retired loads that missed L2 |
| `l2_miss_all` | `l2_rqsts.miss` | all L2 misses (demand, RFO, prefetch) |
| `llc_miss` | `longest_lat_cache.miss` | LLC misses |
| `instructions`, `cycles` | `instructions:u` (fixed counter 0), `cycles:u` | spinning waiters included |
| `ref_cycles` | `cpu_clk_unhalted.ref_tsc:u` (0x0300, fixed counter 2) | unhalted user cycles at the TSC rate |

Counter placement, checked with perf 7.2.5 on this Sapphire Rapids host: 8
general-purpose counters per logical CPU. `cycles:u` occupies one of them (8
general-purpose events plus `cycles:u` multiplex, plus `instructions:u` do not;
fixed counter 1 is presumably held by the NMI watchdog). perf 7.2.5 resolves the
name `ref-cycles` to the programmable `cpu_clk_unhalted.ref_tsc_p` (0x013c), so
the runner names the fixed-counter event explicitly. Six general-purpose plus two
fixed events leave two counters spare. perf's running fraction is recorded per
event and cell (`perf_running`), and a cell with any event below 100 % running is
failed (the raw result is kept).

**Clock.** `ref_tsc` ticks at the TSC rate: 25 MHz crystal × 176/2 = 2.200 GHz
(CPUID 0x15, recorded in the manifest; each cell's measured
`tsc_ticks_per_ns` is used). `effective_ghz` = cycles / ref_cycles × TSC rate:
the mean clock of all threads, weighted by their unhalted user time.
`ref_busy_cpus` = ref_cycles / (TSC ticks of the window): the number of CPUs busy
in user mode (≈ c for spinning locks, minus the combiner's or holder's kernel
time). `tx_s_at_ref` = window tx/s × reference / `effective_ghz` scales throughput
to the reference clock: F under a fixed-clock setup (S1/S2, below), the TSC rate
under S0. It assumes the serial path runs at the process mean clock and all of
its time scales with the clock (kernel and memory time do not, so this
over-corrects fast cells). perf-02 (S0) called this field `tx_s_at_tsc`.
`effective_ghz` is process-wide over all inherited threads; for FC/FC-PQ it is
not the combiner's clock when waiters run at other clocks.

**Per-client clock.** perf stat cannot split a launched command's threads
(`--per-thread` needs `-p`/`-t`) and CPU-wide events need `perf_event_paranoid`
≤ 0, so the runner samples each client CPU's `cpuinfo_avg_freq` (the kernel's
APERF/MPERF over the last 4 ms tick) every 50 ms and `/proc/stat` every 200 ms,
from a thread of the runner on the cell's CPUs, in **every** cell (timed and
perf; the perf-02 overhead run measured no cost, E1/E0 = 1.001). A clock
sample counts if its CPU was ≥ 50 % busy in the enclosing 200 ms interval (an
idle CPU reports a stale or requested value). A client's clock is the mean of its
accepted samples; a cell is **mixed** if the clocks of the clients with ≥ 8
samples differ by more than 1.25× (base 2.2 GHz vs turbo ≥ 3.0 GHz is ≥ 1.36).
Fewer than two such clients (1-client cells, most Mutex/Condvar cells) is
undecidable, not mixed. This is a heuristic: it covers the whole child process,
not only the client phase. In an off-socket check the sampler's mean agreed with
perf's `effective_ghz` within 1-2 %.

**Power setups.** `--prepare-only` requires `--power-setup`; the manifest
records `intel_pstate` (status, `no_turbo`), every cpufreq policy's governor
and min/max, and per run CPU its cpufreq settings and cpuidle states (name,
disable flag, exit latency). The runner has no root; the operator sets the state.

| Setup | Governor, min/max on every policy | Turbo | C6 on run CPUs |
|---|---|---|---|
| `S0` | `schedutil`, cpuinfo range (0.8-3.9 GHz) | on | enabled |
| `S1` | `performance`, min = max = F (`--fixed-ghz`, default 3.0) | on (`no_turbo` = 0) | enabled |
| `S2` | as S1 | on | disabled |

Preparation and every `--smoke`/`--run`/`--perf` refuse a host that differs, and
each cell re-checks after it runs; a change fails the cell. Under S1/S2 a cell
whose clock (perf `effective_ghz`, else the sampler's client mean) is outside
F ± 2 % is flagged `clock_off_target` (kept in the raw rows, counted in the
tables, excluded from `*_not_flagged`). Above base the chip holds F only while
power and thermal budget allow, so `--sustain-probe --power-setup S1 --fixed-ghz
F --cpus …` spins every CPU (one `perf stat` per CPU) and reports each CPU's
effective clock, to choose the highest F that holds. `--check-power` prints the
mismatches. `--variants` restricts a prepared root to a subset.

`--analyze-perf` writes `analysis-perf/summary.{json,md}` with every counter per
committed transaction and per record, and a clock table (effective and client
clocks, spread, mixed and off-target counts, `ref_busy_cpus`, cycles/tx,
`tx_s_at_ref`). Counting covers every request committed while it was enabled,
including those draining after the 2 s window, so the normaliser is
`completed_transactions`/`completed_records`, not the window counts.

## Workflow

Enter the environment and set `MEASUREMENT_LOCK` using [the shared guide](../README.md).
Run from the repository root; CPU/NUMA IDs below are illustrative.

```sh
export BUILD=.worktree/redb-build-01
flock --shared "$MEASUREMENT_LOCK" python3 -m integration.redb.build --output-dir "$BUILD" --jobs 8

flock --exclusive "$MEASUREMENT_LOCK" prlimit --core=0 \
  python3 -m integration.redb.correctness --build-dir "$BUILD" --output-dir "$BUILD/correctness-01"
```

The gate runs every case as a fresh process on a fresh database, for all seven
variants and the opt-in `fc_pq_hn`/`fc_pq_h8` (nine by default; `--variants` restricts),
all writes as closures: exact contents after every request with
close/reopen; shape rejections; duplicate-key abort and the lock being released
after aborts; transaction-ID order, including across gate entry/exit; one gate
per database, public `begin_write` waiting for it and an unrun call reporting
`NotExecuted`; persistent and ephemeral savepoints across the write phase and
created/restored inside closures; 16-writer/2-reader insert stress separately
for `Immediate` and `None` (exact contents, reopen, reads completed during
writes, contiguous IDs); closure `Err` (explicit and redb errors via `?`)
aborting all writes including a table it created, aborts consuming IDs, nested
submission refused; closure panics (typed and string payloads, table handle open
or dropped) re-raised on the requester for every variant with no partial writes
and the lock usable; read-your-own-writes inside a closure; 16-writer/2-reader
transfer stress for both durabilities (every reader snapshot conserves the
total, final and reopened totals conserved, one log row per committed transfer,
an ID-order replay of the committed transfers equals the final balances,
committed + aborted fill the ID range). Every reopen also runs redb's
`check_integrity`, which fails if an aborted or panicked transaction leaked pages
(a negative control that dropped the transaction while panicking, as redb's
`Drop` does, passed the content checks but failed this one). FC/FC-PQ closures
must be observed on a combiner and std_mutex/MCS/U-SCL/upstream_gate/upstream never.
Service charging is checked in the insert stress cases (`stress-*`): each
committed body is charged nonzero service to its requester, the charged total
never exceeds the stress phase's wall ticks (bodies serialised, nothing
double-charged), and in the test_hooks binary it equals the total measured on
the executing threads. A lone FC-PQ requester must take the fast path on all 90
`contents` requests. The test_hooks binary adds in-body occupancy of at most
one, bridge-level combiner evidence, reads proceeding while a writer is paused
inside the lock with a second writer blocked, and SIGABRT fail-stop for a panic
inside a delegated body outside the closure. Test-hook fail-stop children
intentionally abort. Raw records stay under the output directory. The gate
refuses an `--uninstrumented` build.

Upstream redb's own tests also run against the patched tree (on a copy, since
its dev-dependencies are downloaded and its `Cargo.lock` must not change):
`cargo test --release --lib --test basic_tests --test integration_tests
--test multithreading_tests` in a copy of `.worktree/redb-src/redb-3.1.0`.

```sh
export CPUS=16,17,18,19,20,21,22,23 NUMA_NODE=0 RUN=.worktree/redb-smoke-01
flock --shared "$MEASUREMENT_LOCK" taskset -c "$CPUS" python3 -m integration.redb.run \
  --prepare-only --build-dir "$BUILD" --output-root "$RUN" --cpus "$CPUS" --numa-node "$NUMA_NODE" \
  --power-setup S0   # or: --power-setup S1 --fixed-ghz 3.0
flock --exclusive "$MEASUREMENT_LOCK" taskset -c "$CPUS" python3 -m integration.redb.run --smoke --output-root "$RUN"
flock --shared "$MEASUREMENT_LOCK" python3 -m integration.redb.run --analyze-smoke --output-root "$RUN"
```

Preparation re-verifies `build.json` (instrumented, FC-PQ with
`fcpq_fast_path`), snapshots `redb-upstream`/`redb-patched`, and freezes
source/patch/binary hashes and features, the client sweep, CPU topology rows,
the `numactl --show` policy and filesystem. Smoke runs 504 timed 2-second cells
(seven variants × {`all1`, `half1_half64`, `transfer`} × clients {1, 2, 4, 8} ×
both durabilities × three repetitions, variants in randomized order), each a
fresh process with exact-content (inserts) or conservation (transfer)
verification and close/reopen; ≈ 18 min at the 2.16 s per cell of the
redb-internal smoke (336 cells in 12 min, 2026-09-28). Its analysis
(`analysis-smoke/summary.json`, plus `summary.md` and `throughput.png`) lists
`None` first and includes `table` (median [min, max] per durability × cohort ×
clients × variant) and `upstream_gate_vs_upstream`: per durability, cohort and client
count, the median tx/s ratio against the larger relative repeat range of the two
variants. Transfer tx/s counts committed transfers; insufficient-balance aborts
are reported separately (`aborted_transactions`, `aborted_fraction`). Smoke is a
setup and control check, not a performance result.

The formal matrix (seven variants × {`all1`, `half1_half8`, `half1_half64`} at
1, 2, 4, 8 clients × two durabilities × three repetitions = 504 insert cells,
≈ 18 min at 2.16 s per cell; transfer is smoke-only for now) and the 168-cell
perf cohort run only when explicitly requested, on the same prepared root:

```sh
flock --exclusive "$MEASUREMENT_LOCK" taskset -c "$CPUS" python3 -m integration.redb.run --perf --output-root "$RUN"
flock --shared "$MEASUREMENT_LOCK" python3 -m integration.redb.run --analyze-perf --output-root "$RUN"
flock --exclusive "$MEASUREMENT_LOCK" taskset -c "$CPUS" python3 -m integration.redb.run --run --output-root "$RUN"
flock --shared "$MEASUREMENT_LOCK" python3 -m integration.redb.run --analyze-only --output-root "$RUN"
```

Failed cells are never overwritten or re-run in place; populated result
directories are rejected. Use `--numa-node none` at preparation only when
deliberately omitting memory binding, and report it. Capacity guards remain 8
million records per worker and a 512 MiB database file.

## Known limits

- **One closure per transaction.** No group commit (a combiner running several
  closures in one transaction and one fsync would change isolation and needs
  per-closure rollback; redb's `ephemeral_savepoint` refuses dirty transactions).
  No async or interactive transactions: they cannot be a closure run on a
  combiner.
- **Reads outside the lock.** `begin_read` is not delegated or measured; reader
  interference with commit (e.g. freed-page retention) is upstream behaviour.
- **Commit I/O inside the critical section.** Under `Immediate` (the control)
  each transaction holds the lock across fsync, so lock-algorithm differences
  are diluted. `None`, the primary regime, is not a durable-commit claim.
  Close/reopen is not a power-loss test.
- While a gate exists, public `begin_write` blocks; a thread (or closure) that
  holds the gate and calls `begin_write` deadlocks, as a second `begin_write`
  would upstream. Routing public `begin_write` through the lock during delegated
  mode is a recorded follow-up (works for Mutex/MCS; FC/FC-PQ need a policy).
- A failed commit is not rolled back by the body (redb marks the transaction
  completed before commit work, as upstream); the error is returned as `E`.
- A panic raised inside a redb call while redb holds one of its internal mutexes
  (e.g. a user `Value` impl during `open_table`/commit paths) can poison it; the
  following abort may then panic, which aborts the process on the delegated path
  (and unwinds on the caller for `upstream_gate`/`upstream`). Not exercised by the gate.
- **Codegen parity.** The insert closure is monomorphised in the harness crate
  (as for `upstream`), not inside redb as the former fixed body was. Without
  cross-crate LTO this measured 2-4% fewer tx/s for the patched variants than the
  fixed-body build on stable cells (upstream unchanged), so `Cargo.toml` sets
  `lto = "thin"` for every binary; with thin LTO on both builds the six variants
  compared (before `uscl` was added) match (all1/None interleaved A/B, 5 pairs:
  1.001-1.004; fat LTO with one codegen unit: 0.997-1.004). Compare builds only
  with the same profile; results from earlier non-LTO builds are not comparable
  at that precision. The redb-internal numbers quoted here (instrumentation
  overhead, smoke cell time, perf-02 clock data) were measured on the fixed-body
  non-LTO build.
- **CPU frequency.** Under S0 (`schedutil`, turbo), a busy pinned core runs at
  base 2.2 GHz or at 3.0-3.9 GHz, per core, and can change within a process.
  Cores whose clients sleep (U-SCL, rotating Mutex/Condvar hand-off) run at
  0.8-1.1 GHz. Throughput is ≈ proportional to the clock (log-log slope
  0.89-1.10; [perf-02](../../plan/2026-09-28/redb-perf-02-clock.md)). Every
  cell now records the clock. S1/S2 pin it, but only as far as the power
  budget allows (see the off-target flag).
- The service span starts after `begin` returns, so `WriteTransaction`
  construction (inside the critical section) is not charged, identically for
  every variant. Service is TSC ticks, not CPU time: a U-SCL holder that sleeps
  or a body waiting on I/O is still in service.
- **Aborted transfers' service is uncounted.** An insufficient-balance abort
  holds the lock for its reads, debit and rollback, but the harness's
  `Writer::write` returns service only with a committed transaction (`Written`),
  so the `transfer` service metrics (`service_jain`, `worker_service_share`,
  `service_utilization`) cover committed transfers only; aborts are counted in
  `aborted_transactions`.
- One to eight pinned clients per trial (one per CPU, never more client threads
  than CPUs; MCS/FC/FC-PQ spin, the Mutex/Condvar controls block, U-SCL may
  yield or sleep); synthetic key/transfer streams, not a production trace.
