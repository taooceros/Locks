# redb: closure-style delegated write API

Status: implemented 2026-09-29 in workspace `redb-closure`
(`.worktree/jj/redb-closure`); see "Outcome" at the end.

## Goal

Let redb users submit a whole write transaction as a closure that a delegation
lock (Mutex, MCS, FC, FC-PQ) runs, possibly on a combiner:

```rust
let (txn_id, bal) = writer.write(|tx: &mut WriteTransaction| -> Result<u64, MyError> {
    tx.set_durability(Durability::Immediate)?;
    let mut t = tx.open_table(ACCOUNTS)?;
    let bal = t.get("alice")?.map(|v| v.value()).unwrap_or(0);
    if bal < 100 { return Err(MyError::Insufficient); }   // Err  => abort
    t.insert("alice", bal - 100)?;
    Ok(bal)                                               // Ok   => commit
})?;
```

This replaces the fixed-insert body (one `u64 -> u64` table, 1-64 new keys, no
reads), which cannot express read-modify-write or multi-table transactions.

## Contract

- Signature (patched redb, `redb::dlock_private`):
  `F: FnOnce(&mut WriteTransaction) -> Result<R, E> + Send`, `R: Send`,
  `E: Send + From<redb::CommitError>` (commit errors surface as `E`).
  Delegated returns `Result<(u64 /*txn id*/, R), DelegatedWriteError<E>>`
  where the error distinguishes the closure's `E` from `NotExecuted`.
- `&mut WriteTransaction`: the closure may call `set_durability`,
  `set_two_phase_commit`, `set_quick_repair`, savepoint methods, open/delete
  tables. It cannot `commit`/`abort` (both take `self`); the body commits on
  `Ok` and aborts on `Err`. No separate durability argument.
- Borrowing: the requester blocks until its closure finishes, so `F` may borrow
  the caller's stack; no `'static`. Type erasure extends the existing
  `DelegatedCall`/`RawDelegatedCall` context pointer (trampoline + outcome slot).
- Panics: caught inside the body on the executing thread; the transaction is
  dropped (redb aborts it on drop, as upstream); the payload is stored in the
  outcome slot and re-raised on the **requester** with `resume_unwind`. The
  combiner never unwinds and the lock stays usable. Panics in the lock/bridge
  itself still abort the process.
- Documented, not enforced: no `begin_write` or nested submission inside a
  closure (nested submission is refused by the existing `BRIDGE_DEPTH` guard
  and reported as `NotExecuted`); no waiting on other threads' progress (stalls
  the combiner); closures on FC/FC-PQ run on the combiner's thread and see its
  thread-locals. `R: Send` and the lifetime prevent returning table handles.
- `refactored` control: same body under public `begin_write` (tracker
  Mutex/Condvar) on the caller.

## Changes

1. **Patch 0002** (`integration/redb/patches/0002-*.patch`): replace
   `fixed_insert_body`, `FixedInsert`, `FixedInsertTarget`, request validation
   and `FixedInsertError` with the generic closure body and the contract above.
   `DelegatedWriteGate::enter(&Database)` stays one gate per database. Keep
   `dlock_test_hooks` probes (occupancy, pause) in the body; `fail_next` /
   `panic_next` become unnecessary where a closure can return `Err` or panic
   directly; remove what is obsolete. Patch 0001 unchanged unless required.
2. **Harness** (`integration/redb/src/writer.rs`, `main.rs`, `selftest.rs`):
   `Writer::write` takes a closure; the existing fixed insert (1..=64 new
   `u64 -> u64` keys, duplicate => abort) is expressed as a harness closure
   with identical semantics, so existing cohorts are unchanged. Native binary
   runs the same closure against upstream `begin_write`/`commit`. Add a
   `transfer` cohort: accounts table of fixed size; each transaction reads two
   accounts and moves an amount (2 reads + 2 updates), aborting on
   insufficient balance.
3. **Correctness gate** (`correctness.py`, `selftest.rs`): keep every existing
   case, now driven through closures. Add: closure `Err` aborts and releases the
   lock; closure panic re-raised on the requester for every variant, lock
   usable afterwards, transaction aborted (no partial writes); transfer stress
   (16 writers, 2 readers, both durabilities) conserves the total balance,
   including after reopen, and readers always observe a conserved total;
   read-your-own-writes inside a closure; FC/FC-PQ closures observed on a
   combiner.
4. **Build/run** (`build.py`, `run.py`): update patch hashes/provenance as
   needed; add the transfer cohort to smoke.
5. **Docs**: `integration/redb/README.md` (application-logic boundary,
   invariants, known limits) and `TODO.md` entry.

## Out of scope (recorded follow-ups)

- **Public `begin_write` during delegated mode**: still blocks while a gate
  exists. Routing it through the lock works for Mutex/MCS but needs a policy
  for FC/FC-PQ.
- **Group commit** (combiner runs N closures in one transaction, one fsync):
  changes isolation between closures and needs per-closure rollback; redb's
  `ephemeral_savepoint` refuses dirty transactions, so it needs further
  patching. Separate experiment.
- Async / interactive transactions: not expressible as a closure run on a
  combiner.

## Risks

- Trampoline must run `F` exactly once and fill the outcome before the requester
  returns (same invariant the bridge enforces today; unrun call => abort).
- Panic payload is `Box<dyn Any + Send>`; moving it across threads is sound.
  Unwinding must not cross the lock's combine loop.
- Closure work is now inside the critical section; FC-PQ charges it to the
  requester's usage as before.
- Parity: fixed-insert cohorts through the closure path must match the current
  fixed-body build within repeat range before any new result is reported.

## Evaluation

- `cargo build --release` for native/patched/test_hooks; `rustfmt`.
- Correctness gate passes for all six variants; upstream redb tests
  (`--lib`, `basic_tests`, `integration_tests`, `multithreading_tests`) pass on
  a copy of the patched tree.
- Smoke: existing cohorts via closures vs the current fixed-body build
  (`refactored_vs_native` and cross-build ratio within repeat range), plus the
  transfer cohort. Formal matrix only on explicit request.

## Outcome (2026-09-29)

- Implemented as planned. Deviations: patch 0002 renamed
  `0002-closure-write-body.patch`; patch 0001 changed in two comments only
  (they named the removed fixed-insert body); `DelegatedWriteError` also has
  `Begin(TransactionError)` and `AbortFailed { error, abort }` so begin/abort
  failures are not folded into `E`; `DelegatedWriteGate::enter` returns
  `DelegatedModeActive`. After a caught closure panic the body calls
  `abort()` explicitly: redb's `Drop` skips the abort while panicking, which
  leaked pages (a negative control doing that passed content checks but failed
  the new reopen `check_integrity`). `panic_next` kept as the injected
  non-closure panic for the fail-stop case; `fail_next` removed. The transfer
  closure writes the debit before the balance check, so aborts roll back a real
  write; self-transfers are rejected. Transfer is smoke-only (formal matrix
  unchanged).
- Gate 94/94, upstream redb tests 165/165, smoke 108 cells without failures.
- Parity: in default release builds the patched variants run 2-4% below the
  fixed-body build on stable cells (all1/None interleaved A/B, 5 pairs,
  native 1.001); with fat LTO and one codegen unit for both builds all six
  variants match (0.997-1.004). The gap is where the insert body is
  monomorphised (inside redb before, in the harness crate now, as for native),
  not the closure mechanism. Resolved by `lto = "thin"` in the harness release
  profile (all binaries): thin LTO on both builds gives parity 1.001-1.004 over
  the same A/B; clean build of the three binaries (`--jobs 8`) 29.9 s -> 33.9 s,
  incremental after touching `writer.rs` 6.1 s -> 11.5 s (fat/cgu=1: 71.9 s,
  42.8 s). Gate on the thin-LTO build: 94/94.

## Folded into PR #54

The redb-internal rerun harness (workspace `redb-internal`, change `mnkkkmky`;
[redb-internal-rerun](../2026-09-28/redb-internal-rerun.md),
[redb-perf-02-clock](../2026-09-28/redb-perf-02-clock.md)) was ported onto the
closure body, so PR #54 carries both. Ported:

- **`uscl` variant**: U-SCL through the redb bridge on the requester (seven
  variants).
- **Service time measured in `write_body`** (redb feature `dlock_service_time`,
  harness `service_time`): `Served { outcome, service_tsc }` from
  `DelegatedWriteGate::execute_served` and `execute_native_served`; `native`
  measures the identical span around upstream `begin_write`/commit.
- **Client sweep** {1, 2, 4, 8} on 1-8 CPUs (`half1_halfK`: 1 client or an even
  count; `transfer`: any count).
- **Perf cohort** (`run.py --perf`, `--analyze-perf`): user-mode `perf stat`
  over the client phase, no multiplexing.
- **Power setups** S0/S1/S2 (`--power-setup`, `--fixed-ghz`, `--check-power`,
  `--sustain-probe`); the runner refuses a host not in the setup.
- **Clock normalisation**: `ref_tsc` effective clock, per-cell
  `tsc_ticks_per_ns`, sampler client clocks, `tx_s_at_ref`, `clock_off_target`
  and `mixed_clock` flags.
- **FC-PQ fast path**: harness features `fcpq_fast_path` and
  `fcpq_fast_path_stat`, per-worker `fast_path_hits`.

Hook decision: the service span sits inside the patched closure body
(`write_body`): `rdtscp` once `begin` has returned and once commit or abort has
returned, on the thread that ran the body (the combiner for FC/FC-PQ), charged
to the requester. It therefore includes the closure and commit and excludes
admission waiting, the delegation lock and the bridge, the same span as the
former fixed-insert body; panicked closures are not charged.
