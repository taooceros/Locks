# redb: delegation locks inside the write path

This experiment patches redb 3.1.0 so that a delegation lock (Mutex, MCS, FC,
FC-PQ) **is** redb's write-serialisation mechanism, following the UpScaleDB
pattern in [../upscaledb/](../upscaledb/README.md): one extracted write body,
native/refactored/bridge controls, a pinned source with numbered patches, a
real-database correctness gate and the existing fresh-process runner.

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
  by an MCS hand-off. The primary `mcs` variant already runs the identical fixed
  body under MCS on the requesting thread; with this narrow request shape the
  code between begin and commit is the same in both designs, so the extra variant
  would mostly re-measure the same critical section while adding a second patch
  surface to `start_write_transaction`.
- If it is ever needed (e.g. to measure unchanged-API applications with arbitrary
  caller code inside the transaction), it must be a separately labelled variant
  (`admission_mcs`) with its own cohort, never mixed into the primary comparison.

## Application-logic boundary

Each submission is one **fixed-shape write request**: 1–64 new `u64 -> u64`
records for one pre-existing table, committed with exactly one durability mode
(`Immediate` or `None`). Patched redb contains one internal body,
`fixed_insert_body`, that performs begin → `set_durability` → `open_table` →
inserts → commit, or an explicit abort when a key already exists. Upstream redb
code performs the storage, B-tree, allocator and commit work unchanged; only the
writer admission differs:

- **refactored**: `FixedInsertTarget::execute_native` runs the body; its begin
  step is the unchanged public `begin_write`, i.e. redb's original tracker
  `Mutex<State>` + `live_write_transaction_available` Condvar.
- **delegated** (`bridge_mutex`, `mcs`, `fc`, `fc_pq`): `DelegatedWriteGate::execute`
  validates the request on the caller, then hands the *same* body to the lock
  through a synchronous submit closure. Mutex and MCS run it on the requesting
  thread; FC and FC-PQ may run it on a combiner.

The critical section is therefore the whole transaction, **including commit I/O**.
Anything outside this shape (empty, >64 records, other types or tables, closures)
is rejected before submission, never approximated. Arbitrary caller closures are
out of scope. Read transactions (`Database::begin_read`) **stay outside every
write lock and the bridge**; they register with redb's tracker exactly as upstream.

### Invariants on the delegated path

| redb rule | How it holds without the tracker's writer lock |
|---|---|
| One live writer | `DelegatedWriteGate::enter` occupies the tracker's writer slot once (waiting on the unchanged Condvar, like any writer), so public `begin_write` callers wait until the gate is dropped. Per request, the body claims a delegated writer slot by atomic compare-exchange and **panics (→ process abort) if another writer is live**. |
| Transaction-ID order | Entering the gate copies the tracker's ID counter; the body allocates `last + 1` inside the delegation lock; leaving writes it back. Aborted writes consume IDs exactly as upstream. |
| Reads | Untouched: readers take the last committed ID via the tracker's state mutex, as upstream. The commit code's reader/savepoint bookkeeping calls are unchanged. |
| Savepoints | The body creates none; commit consults the tracker's savepoint/reader state unchanged, so savepoints taken before or after a delegated phase stay valid. |

Per request, the delegated path never takes the tracker's `Mutex<State>` or waits
on its Condvar for admission (it cannot: the gate already holds that slot). A
panic inside a delegated body is caught at the body boundary and aborts the
process; it never unwinds across a combiner. Every error path (duplicate key,
redb error, injected error) aborts the transaction inside the body and returns
normally, so the lock is released. Transaction and guard objects are created and
dropped inside the body on the executing thread; only the plain request (a
borrowed slice and a durability) and plain outcome cross threads.

## What is patched

`build.py` verifies the crates.io archive `redb-3.1.0.crate` against its pinned
SHA-256 (the same checksum `Cargo.lock` locks for the native build), extracts it,
applies these patches in order and places the result at the fixed Cargo path
dependency `.worktree/redb-src/redb-3.1.0` (generated, ignored):

| Patch | Files | Change |
|---|---|---|
| `patches/0001-delegated-writer-admission.patch` | `transaction_tracker.rs`, `db.rs`, `transactions.rs` | Delegated mode (enter/exit), delegated start/end writer slot with assertion, delegated `TransactionGuard`, `Database::begin_delegated_write` (a copy of `begin_write` with delegated admission), transaction-ID accessor |
| `patches/0002-fixed-insert-write-body.patch` | new `dlock_private.rs`, `lib.rs`, `Cargo.toml` | The shared body, request validation, `FixedInsertTarget`, `DelegatedWriteGate`, `DelegatedCall`, and the `dlock_test_hooks` feature (correctness probes only) |

`build.json` records the crate SHA-256 and origin, upstream and patched tree
hashes, each patch hash, harness/libdlock source hashes, `rustc -vV`, git state
and each binary's SHA-256. An existing patched tree that differs from a fresh
application stops the build.

## Variants

| Variant | Binary | Write serialisation |
|---|---|---|
| `native` | `redb-native` | Upstream crates.io redb, untouched; public-API sequence identical to the body |
| `refactored` | `redb-patched` | Shared body under redb's original Mutex/Condvar |
| `bridge_mutex` | `redb-patched` | Shared body via the bridge, `std::sync::Mutex` (redb's primitive type; the tracker's own mutex cannot be borrowed because commit re-enters it) |
| `mcs` | `redb-patched` | Shared body via the bridge, libdlock MCS, on the requester |
| `fc` | `redb-patched` | Shared body via the bridge, libdlock FC, possibly on a combiner |
| `fc_pq` | `redb-patched` | Shared body via the bridge, libdlock FC-PQ, possibly on a combiner |

`redb-test_hooks` (patched redb with `dlock_test_hooks`) is used only by the
correctness gate and is never timed.

| File | Purpose |
|---|---|
| `patches/` | Numbered patches against redb 3.1.0 |
| `build.py` | Pinned source preparation, three `--locked` builds, `build.json` provenance |
| `correctness.py` | Real-database gate for every variant |
| `run.py` | Prepare/smoke/formal fresh-process runner and offline analysis |
| `src/writer.rs` | Variants and the bridge (`Bridge`, the only code that touches libdlock) |
| `src/main.rs`, `src/selftest.rs` | Timed workload and self-test cases |

## Workflow

Enter the environment and set `MEASUREMENT_LOCK` using [the shared guide](../README.md).
Run from the repository root; CPU/NUMA IDs below are illustrative.

```sh
export BUILD=.worktree/redb-build-01
flock --shared "$MEASUREMENT_LOCK" python3 -m integration.redb.build --output-dir "$BUILD" --jobs 8

flock --exclusive "$MEASUREMENT_LOCK" prlimit --core=0 \
  python3 -m integration.redb.correctness --build-dir "$BUILD" --output-dir "$BUILD/correctness-01"
```

The gate runs every case as a fresh process on a fresh database: exact contents
after every request with close/reopen; shape rejections; duplicate-key abort and
the lock being released after aborts and injected errors; transaction-ID order,
including across gate entry/exit; one gate per database and public `begin_write`
waiting for it; persistent and ephemeral savepoints across the write phase;
16-writer/2-reader stress separately for `Immediate` and `None` (exact contents,
reopen, reads completed during writes, contiguous IDs, in-body occupancy of at
most one, FC/FC-PQ bodies observed on combiners and Mutex/MCS never); reads
proceeding while a writer is paused inside the lock with a second writer blocked;
and SIGABRT fail-stop for a panic inside a delegated body. Test-hook children
intentionally abort. Raw records stay under the output directory.

Upstream redb's own tests also run against the patched tree (on a copy, since
its dev-dependencies are downloaded and its `Cargo.lock` must not change):
`cargo test --release --lib --test basic_tests --test integration_tests
--test multithreading_tests` in a copy of `.worktree/redb-src/redb-3.1.0`.

```sh
export CPUS=16,17,18,19,20,21,22,23 NUMA_NODE=0 RUN=.worktree/redb-smoke-01
flock --shared "$MEASUREMENT_LOCK" taskset -c "$CPUS" python3 -m integration.redb.run \
  --prepare-only --build-dir "$BUILD" --output-root "$RUN" --cpus "$CPUS" --numa-node "$NUMA_NODE"
flock --exclusive "$MEASUREMENT_LOCK" taskset -c "$CPUS" python3 -m integration.redb.run --smoke --output-root "$RUN"
flock --shared "$MEASUREMENT_LOCK" python3 -m integration.redb.run --analyze-smoke --output-root "$RUN"
```

Preparation re-verifies `build.json`, snapshots `redb-native`/`redb-patched`,
and freezes source/patch/binary hashes, CPU topology rows, the `numactl --show`
policy and filesystem. Smoke runs 72 timed 2-second cells (six variants, `all1`
and `half1_half64`, both durabilities, three repetitions, randomized order), each
a fresh process with exact-content verification and close/reopen. Its analysis
(`analysis-smoke/summary.json`) includes `refactored_vs_native`: per cohort and
durability, the median tx/s ratio against the larger relative repeat range of the
two variants. Smoke is a setup and control check, not a performance result.

The formal matrix (six variants × three cohorts × two durabilities × three
repetitions = 108 cells) runs only when explicitly requested:

```sh
flock --exclusive "$MEASUREMENT_LOCK" taskset -c "$CPUS" python3 -m integration.redb.run --run --output-root "$RUN"
flock --shared "$MEASUREMENT_LOCK" python3 -m integration.redb.run --analyze-only --output-root "$RUN"
```

Failed cells are never overwritten or re-run in place; populated result
directories are rejected. Use `--numa-node none` at preparation only when
deliberately omitting memory binding, and report it. Capacity guards remain 8
million records per worker and a 512 MiB database file.

## Known limits

- **Narrow request shape.** Only fixed-width `u64 -> u64` inserts into one table,
  1–64 records, one durability per request. No updates, deletes, reads inside the
  write, multi-table writes or caller closures; no multi-request batching.
- **Reads outside the lock.** `begin_read` is not delegated or measured; reader
  interference with commit (e.g. freed-page retention) is upstream behaviour.
- **Commit I/O inside the critical section.** Under `Immediate` each request holds
  the lock across fsync, so lock-algorithm differences are diluted; `None` is a
  different durability regime, not a durable-commit claim. Close/reopen is not a
  power-loss test.
- While a gate exists, public `begin_write` blocks; a thread that holds the gate
  and calls `begin_write` deadlocks, as a second `begin_write` would upstream.
- No profile (service-time instrumentation) build is included in this cutover.
- Eight pinned clients per trial; synthetic key streams, not a production trace.
