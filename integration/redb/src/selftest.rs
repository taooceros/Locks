//! Real-database self-tests for the correctness gate (`correctness.py`).
//! Each case uses a fresh database file and prints one JSON object on success.
//! Every write goes through a closure submitted to the variant's `Writer`.
use std::any::Any;
use std::collections::BTreeMap;
use std::error::Error;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
#[cfg(feature = "patched")]
use std::time::Instant;

use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
    TableHandle, WriteTransaction,
};
use serde_json::{json, Value};

use crate::writer::{
    insert_body, transfer_body, Variant, WriteError, Writer, Written, MAX_RECORDS_PER_REQUEST,
    TABLE,
};
use crate::{
    conserved_balances, create_database, key, option, parse_durability, records, seed_accounts,
    value, Transfers, INITIAL_BALANCE,
};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const SEED: u64 = 0x5EED_0F_7E57;
const DEADLINE: Duration = Duration::from_secs(20);
/// Scratch table created only inside aborted closures; must never exist.
const SCRATCH: TableDefinition<u64, u64> = TableDefinition::new("scratch_never_committed");
/// Transfer stress: one log row per committed transfer (double-run detector).
const TRANSFER_LOG: TableDefinition<u64, u64> = TableDefinition::new("transfer_log");
const STRESS_ACCOUNTS: u64 = 64;

macro_rules! ensure {
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err(format!($($message)+).into());
        }
    };
}

pub fn run(args: &[String]) -> TestResult {
    let case = option("--self-test", args)?;
    let variant = Variant::parse(&option("--variant", args)?)?;
    let path = PathBuf::from(option("--database", args)?);
    let db = create_database(&path)?;
    let durability = || parse_durability(&option("--durability", args)?);
    let details = match case.as_str() {
        "contents" => contents(db, &path, variant)?,
        "stress" => stress(db, &path, variant, durability()?)?,
        "errors" => errors(db, &path, variant)?,
        "savepoints" => savepoints(db, &path, variant)?,
        "closure-error" => closure_error(db, &path, variant)?,
        "closure-panic" => closure_panic(db, &path, variant)?,
        "read-own-writes" => read_own_writes(db, &path, variant)?,
        "transfer" => transfer_stress(db, &path, variant, durability()?)?,
        #[cfg(feature = "test_hooks")]
        "paused-writer" => hooks::paused_writer(db, variant)?,
        #[cfg(feature = "test_hooks")]
        "body-panic" => hooks::body_panic(db, variant)?,
        _ => return Err(format!("unknown self-test for this binary: {case}").into()),
    };
    println!(
        "{}",
        json!({"self_test": case, "variant": variant.name(), "status": "ok",
               "test_hooks": cfg!(feature = "test_hooks"), "details": details})
    );
    Ok(())
}

/// Exact table contents, compared in key order.
fn verify_exact(db: &Database, expected: &BTreeMap<u64, u64>) -> TestResult {
    let read = db.begin_read()?;
    let table = read.open_table(TABLE)?;
    let mut expected_iter = expected.iter();
    for pair in table.iter()? {
        let (k, v) = pair?;
        let (k, v) = (k.value(), v.value());
        match expected_iter.next() {
            Some((&ek, &ev)) if ek == k && ev == v => {}
            Some((&ek, &ev)) => {
                return Err(format!("content mismatch: read {k}={v}, expected {ek}={ev}").into())
            }
            None => return Err(format!("unexpected extra key {k}").into()),
        }
    }
    ensure!(
        expected_iter.next().is_none(),
        "missing keys: table has {} of {}",
        table.len()?,
        expected.len()
    );
    let tables: Vec<String> = read.list_tables()?.map(|t| t.name().to_string()).collect();
    ensure!(
        !tables.iter().any(|t| t == SCRATCH.name()),
        "a table created by an aborted closure exists"
    );
    Ok(())
}

/// Reopen, compare contents, then redb's integrity check: a rolled-back
/// transaction that leaked pages or left a stale root is reported as a repair.
fn reopen_exact(path: &PathBuf, expected: &BTreeMap<u64, u64>) -> TestResult {
    let mut reopened = Database::open(path)?;
    verify_exact(&reopened, expected)?;
    ensure!(
        reopened.check_integrity()?,
        "integrity check after reopen had to repair the database"
    );
    Ok(())
}

fn write_ok(
    writer: &Writer<'_>,
    request: &[(u64, u64)],
    durability: Durability,
    expected: &mut BTreeMap<u64, u64>,
) -> TestResult<Option<u64>> {
    let written = writer
        .insert(request, durability)
        .map_err(|e| e.to_string())?;
    expected.extend(request.iter().copied());
    Ok(written.transaction_id)
}

/// Transaction IDs must be strictly increasing in submission order.
fn ensure_increasing(ids: &[Option<u64>]) -> TestResult {
    let ids: Vec<u64> = ids.iter().flatten().copied().collect();
    ensure!(
        ids.windows(2).all(|pair| pair[0] < pair[1]),
        "transaction IDs not increasing: {ids:?}"
    );
    Ok(())
}

/// Run `work` on another thread; fail instead of hanging if the lock leaked.
/// The thread is joined explicitly (pthread_join) on success, so per-thread
/// lock state (U-SCL's pthread TLS) is torn down before the lock can be dropped.
fn with_deadline<T: Send>(label: &str, work: impl FnOnce() -> T + Send) -> TestResult<T> {
    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let handle = scope.spawn(move || {
            let _ = sender.send(work());
        });
        let outcome = receiver
            .recv_timeout(DEADLINE)
            .map_err(|_| format!("{label}: no progress within {DEADLINE:?}; lock not released?"))?;
        handle
            .join()
            .map_err(|_| format!("{label}: worker panicked"))?;
        Ok(outcome)
    })
}

/// Wrap a closure so it counts executions on a thread other than its requester
/// (an FC/FC-PQ combiner). Works in every build; no test hooks needed.
fn probed<'a, R>(
    remote: &'a AtomicU64,
    f: impl FnOnce(&mut WriteTransaction) -> Result<R, WriteError> + Send + 'a,
) -> impl FnOnce(&mut WriteTransaction) -> Result<R, WriteError> + Send + 'a {
    let requester = thread::current().id();
    move |txn| {
        if thread::current().id() != requester {
            remote.fetch_add(1, Ordering::Relaxed);
        }
        f(txn)
    }
}

/// FC/FC-PQ must have run closures on a combiner; the others never may.
fn check_remote(variant: Variant, remote: u64, what: &str) -> TestResult {
    if variant.combining() {
        ensure!(remote > 0, "no {what} ran on a combiner");
    } else {
        ensure!(
            remote == 0,
            "{remote} {what} ran on a thread other than the requester"
        );
    }
    Ok(())
}

/// Commit an empty transaction and return its ID (patched variants).
fn marker(writer: &Writer<'_>) -> TestResult<Option<u64>> {
    Ok(writer
        .write(|_| Ok(()))
        .map_err(|e| e.to_string())?
        .transaction_id)
}

fn contents(db: Database, path: &PathBuf, variant: Variant) -> TestResult<Value> {
    let mut expected = BTreeMap::new();
    let mut ids = Vec::new();
    let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
    let mut next = 0;
    let fast_path_hits;
    {
        let writer = Writer::new(&db, variant)?;
        for i in 0..90 {
            let count = [1, 8, 64][i % 3];
            // Alternate so every size is written with both durability modes.
            let durability = if (i / 3) % 2 == 0 {
                Durability::Immediate
            } else {
                Durability::None
            };
            let request = records(&mut buffer, 0, next, count, SEED);
            ids.push(write_ok(&writer, request, durability, &mut expected)?);
            next += count as u64;
            verify_exact(&db, &expected)?;
        }
        // A lone requester never finds another request pending, so every FC-PQ
        // request must take the E0(b) fast path when it is compiled in.
        fast_path_hits = writer.fast_path_hits();
        if variant.is_fc_pq() && cfg!(feature = "fcpq_fast_path") {
            ensure!(
                fast_path_hits.is_some() || !cfg!(feature = "fcpq_fast_path_stat"),
                "fast-path counter missing"
            );
            if let Some(hits) = fast_path_hits {
                ensure!(
                    hits == 90,
                    "lone FC-PQ requester took the fast path {hits}/90 times"
                );
            }
        }
    }
    ensure_increasing(&ids)?;
    if variant != Variant::Upstream {
        let ids: Vec<u64> = ids.iter().flatten().copied().collect();
        ensure!(
            ids.windows(2).all(|p| p[1] == p[0] + 1),
            "IDs not contiguous without aborts: {ids:?}"
        );
    }
    drop(db);
    reopen_exact(path, &expected)?;
    Ok(
        json!({"requests": 90, "records": expected.len(), "reopened_exact": true,
              "fast_path_hits": fast_path_hits}),
    )
}

fn stress(
    db: Database,
    path: &PathBuf,
    variant: Variant,
    durability: Durability,
) -> TestResult<Value> {
    const WRITERS: usize = 16;
    const READERS: usize = 2;
    let per_writer: u64 = if matches!(durability, Durability::Immediate) {
        200
    } else {
        600
    };
    let done = AtomicBool::new(false);
    let reads_during_writes = AtomicU64::new(0);
    let remote = AtomicU64::new(0);
    let writer = Writer::new(&db, variant)?;
    let wall_begin = crate::tsc();
    let (per_writer_written, reader_errors) = thread::scope(|scope| {
        let readers: Vec<_> = (0..READERS)
            .map(|r| {
                let (db, done, reads) = (&db, &done, &reads_during_writes);
                scope.spawn(move || -> Result<(), String> {
                    let mut last_len = 0;
                    let mut round = r;
                    while !done.load(Ordering::Acquire) {
                        let read = db.begin_read().map_err(|e| e.to_string())?;
                        let table = read.open_table(TABLE).map_err(|e| e.to_string())?;
                        let len = table.len().map_err(|e| e.to_string())?;
                        if len < last_len {
                            return Err(format!("snapshot shrank from {last_len} to {len}"));
                        }
                        last_len = len;
                        // One writer's keys in a snapshot: a contiguous prefix, exact values.
                        let owner = round % WRITERS;
                        let mut expect = 0;
                        for pair in table
                            .range(key(owner, 0)..key(owner + 1, 0))
                            .map_err(|e| e.to_string())?
                        {
                            let (k, v) = pair.map_err(|e| e.to_string())?;
                            if k.value() != key(owner, expect)
                                || v.value() != value(SEED, k.value())
                            {
                                return Err(format!(
                                    "snapshot of writer {owner} not a committed prefix at {expect}"
                                ));
                            }
                            expect += 1;
                        }
                        if !done.load(Ordering::Acquire) {
                            reads.fetch_add(1, Ordering::Relaxed);
                        }
                        round += 1;
                    }
                    Ok(())
                })
            })
            .collect();
        let writers: Vec<_> = (0..WRITERS)
            .map(|w| {
                let (writer, remote) = (&writer, &remote);
                scope.spawn(move || -> Result<Vec<Written<()>>, String> {
                    let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
                    let mut next = 0;
                    let mut written = Vec::new();
                    for i in 0..per_writer as usize {
                        let count = [1, 8, 64][(w + i) % 3];
                        let request = records(&mut buffer, w, next, count, SEED);
                        written.push(
                            writer
                                .write(probed(remote, insert_body(request, durability)))
                                .map_err(|e| format!("writer {w}: {e}"))?,
                        );
                        next += count as u64;
                    }
                    Ok(written)
                })
            })
            .collect();
        let ids: Vec<_> = writers
            .into_iter()
            .map(|h| h.join().unwrap_or(Err("writer panic".into())))
            .collect();
        done.store(true, Ordering::Release);
        let reader_errors: Vec<_> = readers
            .into_iter()
            .filter_map(|h| h.join().unwrap_or(Err("reader panic".into())).err())
            .collect();
        (ids, reader_errors)
    });
    let wall_tsc = crate::tsc() - wall_begin;
    ensure!(
        reader_errors.is_empty(),
        "reader failures: {reader_errors:?}"
    );
    let written = per_writer_written
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let worker_ids: Vec<Vec<Option<u64>>> = written
        .iter()
        .map(|w| w.iter().map(|x| x.transaction_id).collect())
        .collect();
    let service: Vec<Vec<u64>> = written
        .iter()
        .map(|w| w.iter().map(|x| x.service_tsc).collect())
        .collect();
    let service = check_service(&service, wall_tsc, true)?;
    let mut expected = BTreeMap::new();
    let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
    for w in 0..WRITERS {
        let mut next = 0;
        for i in 0..per_writer as usize {
            let count = [1, 8, 64][(w + i) % 3];
            expected.extend(records(&mut buffer, w, next, count, SEED).iter().copied());
            next += count as u64;
        }
    }
    verify_exact(&db, &expected)?;
    let reads = reads_during_writes.load(Ordering::Relaxed);
    ensure!(
        reads > 0,
        "no read transaction completed while writers were active"
    );
    let mut id_check = "not exposed by upstream redb";
    if variant != Variant::Upstream {
        for ids in &worker_ids {
            ensure_increasing(ids)?;
        }
        let mut all: Vec<u64> = worker_ids.iter().flatten().flatten().copied().collect();
        all.sort_unstable();
        ensure!(
            all.len() == WRITERS * per_writer as usize,
            "missing transaction IDs"
        );
        ensure!(
            all.windows(2).all(|p| p[1] == p[0] + 1),
            "transaction IDs not unique and contiguous"
        );
        id_check = "unique, contiguous, per-writer increasing";
    }
    let remote_closures = remote.load(Ordering::Relaxed);
    check_remote(variant, remote_closures, "closure")?;
    hook_checks(variant)?;
    drop(writer);
    drop(db);
    reopen_exact(path, &expected)?;
    Ok(
        json!({"writers": WRITERS, "closures_run_on_other_thread": remote_closures,
              "bodies_run_on_other_thread": remote_executions(), "transactions": WRITERS as u64 * per_writer,
              "service": service,
              "records": expected.len(), "durability": format!("{durability:?}"),
              "reads_during_writes": reads, "transaction_ids": id_check,
              "max_body_occupancy": occupancy(), "reopened_exact": true}),
    )
}

/// Service charging over a stress phase: every committed body returned nonzero
/// service to its requester, and the total cannot exceed the phase's wall
/// ticks (bodies are serialised, nothing double-charged). With test hooks the
/// requester total must equal the total measured on the executing threads when
/// every body of the phase reached a requester (`complete`), else not exceed it
/// (aborted transfers are executed but return no service).
fn check_service(per_writer: &[Vec<u64>], wall_tsc: u64, complete: bool) -> TestResult<Value> {
    let service: Vec<u64> = per_writer.iter().map(|w| w.iter().sum()).collect();
    let total: u64 = service.iter().sum();
    if cfg!(feature = "service_time") {
        ensure!(
            per_writer.iter().flatten().all(|&ticks| ticks > 0),
            "a committed body was charged no service time"
        );
        ensure!(
            total <= wall_tsc,
            "charged service {total} exceeds wall {wall_tsc} ticks: bodies overlapped or were double-charged"
        );
    }
    #[cfg(feature = "test_hooks")]
    {
        let executed = redb::dlock_private::test_hooks::service_tsc_total();
        ensure!(
            if complete {
                total == executed
            } else {
                total <= executed
            },
            "service charged to requesters {total} vs executed {executed} (complete: {complete})"
        );
    }
    let _ = complete;
    Ok(json!({"tsc_per_writer": service, "over_wall": total as f64 / wall_tsc as f64}))
}

/// Test-hook build: at most one body between begin and commit; bridge-level
/// combiner evidence agrees with the closure-level probe.
fn hook_checks(variant: Variant) -> TestResult {
    #[cfg(feature = "test_hooks")]
    {
        let max = redb::dlock_private::test_hooks::max_occupancy();
        ensure!(max == 1, "max writers observed inside the body: {max}");
        check_remote(variant, crate::writer::remote_executions(), "bridge call")?;
    }
    let _ = variant;
    Ok(())
}

fn remote_executions() -> Value {
    #[cfg(feature = "test_hooks")]
    return json!(crate::writer::remote_executions());
    #[cfg(not(feature = "test_hooks"))]
    Value::Null
}

fn occupancy() -> Value {
    #[cfg(feature = "test_hooks")]
    return json!(redb::dlock_private::test_hooks::max_occupancy());
    #[cfg(not(feature = "test_hooks"))]
    Value::Null
}

fn errors(db: Database, path: &PathBuf, variant: Variant) -> TestResult<Value> {
    let mut expected = BTreeMap::new();
    let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
    let mut next = 0;
    let mut checks = Vec::new();
    {
        let writer = Writer::new(&db, variant)?;
        let mut write =
            |count: usize, expected: &mut BTreeMap<u64, u64>| -> TestResult<Option<u64>> {
                let request = records(&mut buffer, 0, next, count, SEED);
                next += count as u64;
                write_ok(&writer, request, Durability::Immediate, expected)
            };
        let first = write(1, &mut expected)?;
        // Shape rejections happen before submission: no lock, no transaction ID.
        let oversized: Vec<(u64, u64)> = (0..=MAX_RECORDS_PER_REQUEST as u64)
            .map(|i| (key(1, i), value(SEED, key(1, i))))
            .collect();
        ensure!(
            matches!(
                writer.insert(&[], Durability::Immediate),
                Err(WriteError::Rejected(_))
            ),
            "empty request accepted"
        );
        ensure!(
            matches!(
                writer.insert(&oversized, Durability::None),
                Err(WriteError::Rejected(_))
            ),
            "65-record request accepted"
        );
        let second = write(8, &mut expected)?;
        if let (Some(a), Some(b)) = (first, second) {
            ensure!(
                b == a + 1,
                "rejected requests consumed transaction IDs: {a} -> {b}"
            );
        }
        verify_exact(&db, &expected)?;
        checks.push("rejections");

        // Duplicate key after fresh inserts: the whole transaction is aborted.
        let existing = *expected.keys().next().expect("seeded");
        let partial = [(key(2, 0), 1), (key(2, 1), 2), (existing, 3)];
        ensure!(
            matches!(writer.insert(&partial, Durability::Immediate), Err(WriteError::Duplicate(k)) if k == existing),
            "duplicate key not reported"
        );
        ensure!(
            matches!(
                writer.insert(&[(key(2, 5), 1), (key(2, 5), 2)], Durability::None),
                Err(WriteError::Duplicate(_))
            ),
            "in-request duplicate not reported"
        );
        verify_exact(&db, &expected)?;
        let third = write(1, &mut expected)?;
        if let (Some(b), Some(c)) = (second, third) {
            // Aborted transactions consume IDs exactly as upstream redb's tracker does.
            ensure!(
                c == b + 3,
                "aborted transactions changed ID order: {b} -> {c}"
            );
        }
        checks.push("duplicate-abort");

        // Abort releases the lock: alternate aborting and committing threads.
        for round in 0..50_u64 {
            let dup = [(existing, round)];
            let outcome =
                with_deadline("aborting writer", || writer.insert(&dup, Durability::None))?;
            ensure!(
                matches!(outcome, Err(WriteError::Duplicate(_))),
                "round {round}: duplicate not reported"
            );
            let fresh = [(key(3, round), value(SEED, key(3, round)))];
            with_deadline("writer after abort", || {
                writer.insert(&fresh, Durability::None)
            })?
            .map_err(|e| format!("round {round}: {e}"))?;
            expected.extend(fresh);
        }
        verify_exact(&db, &expected)?;
        checks.push("abort-releases-lock");
    }
    if variant.delegated() {
        errors_gated(&db, &mut expected)?;
        checks.push("single-gate");
        checks.push("public-writer-waits-for-gate");
        checks.push("id-order-across-gate");
    }
    drop(db);
    reopen_exact(path, &expected)?;
    Ok(json!({"checks": checks, "records": expected.len(), "reopened_exact": true}))
}

#[cfg(feature = "upstream")]
fn errors_gated(_: &Database, _: &mut BTreeMap<u64, u64>) -> TestResult {
    unreachable!("upstream has no gate")
}

#[cfg(feature = "patched")]
fn errors_gated(db: &Database, expected: &mut BTreeMap<u64, u64>) -> TestResult {
    use redb::dlock_private::{execute_upstream_gate, DelegatedWriteError, DelegatedWriteGate};
    let one = |k: u64| [(k, value(SEED, k))];
    let tracker_write = |k: u64| -> TestResult<u64> {
        let records = one(k);
        let (id, ()) = execute_upstream_gate(db, insert_body(&records, Durability::Immediate))
            .map_err(|e| e.to_string())?;
        Ok(id)
    };
    let before = tracker_write(key(4, 0))?;
    expected.insert(key(4, 0), value(SEED, key(4, 0)));
    let gate = DelegatedWriteGate::enter(db)?;
    ensure!(
        DelegatedWriteGate::enter(db).is_err(),
        "second gate on one database accepted"
    );
    let mut delegated = Vec::new();
    for i in 1..4 {
        let k = key(4, i);
        let records = one(k);
        let (id, ()) = gate
            .execute(insert_body(&records, Durability::None), |call| call.run())
            .map_err(|e| e.to_string())?;
        delegated.push(id);
        expected.insert(k, value(SEED, k));
    }
    ensure!(
        delegated[0] == before + 1 && delegated.windows(2).all(|p| p[1] == p[0] + 1),
        "IDs across gate entry not continuous: {before} then {delegated:?}"
    );
    // A submit closure that drops the call unrun: NotExecuted, no ID consumed.
    let unrun = one(key(6, 0));
    let outcome = gate.execute(insert_body(&unrun, Durability::None), |call| drop(call));
    ensure!(
        matches!(outcome, Err(DelegatedWriteError::NotExecuted)),
        "unrun call not reported as NotExecuted"
    );
    // A public writer waits on the tracker's slot while the gate holds it.
    let public_done = AtomicBool::new(false);
    thread::scope(|scope| -> TestResult {
        let public = scope.spawn(|| -> Result<(), String> {
            let txn = db.begin_write().map_err(|e| e.to_string())?;
            {
                let mut table = txn.open_table(TABLE).map_err(|e| e.to_string())?;
                table
                    .insert(key(5, 0), value(SEED, key(5, 0)))
                    .map_err(|e| e.to_string())?;
            }
            txn.commit().map_err(|e| e.to_string())?;
            public_done.store(true, Ordering::Release);
            Ok(())
        });
        thread::sleep(Duration::from_millis(300));
        ensure!(
            !public_done.load(Ordering::Acquire),
            "public begin_write ran while the gate was held"
        );
        drop(gate);
        let start = Instant::now();
        while !public.is_finished() {
            ensure!(
                start.elapsed() < DEADLINE,
                "public writer not admitted after the gate was dropped"
            );
            thread::sleep(Duration::from_millis(5));
        }
        public.join().map_err(|_| "public writer panicked")??;
        Ok(())
    })?;
    expected.insert(key(5, 0), value(SEED, key(5, 0)));
    let after = tracker_write(key(4, 9))?;
    expected.insert(key(4, 9), value(SEED, key(4, 9)));
    // last delegated ID, then the public writer, then this tracker writer.
    ensure!(
        after == delegated[2] + 2,
        "IDs across gate exit not continuous: {delegated:?} then {after}"
    );
    verify_exact(db, expected)
}

/// Persistent and ephemeral savepoints taken before a write phase remain valid
/// across it (delegated writes for bridge variants) and restore exactly; then
/// savepoints created and restored inside closures.
fn savepoints(db: Database, path: &PathBuf, variant: Variant) -> TestResult<Value> {
    let mut expected = BTreeMap::new();
    let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
    let mut next = 0;
    {
        let writer = Writer::new(&db, variant)?;
        for _ in 0..4 {
            let request = records(&mut buffer, 0, next, 64, SEED);
            write_ok(&writer, request, Durability::Immediate, &mut expected)?;
            next += 64;
        }
    }
    // Savepoints are created through the public API before the write phase.
    let persistent_contents = expected.clone();
    let persistent = {
        let txn = db.begin_write()?;
        let id = txn.persistent_savepoint()?;
        txn.commit()?;
        id
    };
    {
        let writer = Writer::new(&db, variant)?;
        let request = records(&mut buffer, 0, next, 8, SEED);
        write_ok(&writer, request, Durability::Immediate, &mut expected)?;
        next += 8;
    }
    let ephemeral_contents = expected.clone();
    let ephemeral = {
        let txn = db.begin_write()?;
        let savepoint = txn.ephemeral_savepoint()?;
        txn.commit()?;
        savepoint
    };
    {
        let writer = Writer::new(&db, variant)?;
        for i in 0..200 {
            let count = [1, 8, 64][i % 3];
            let durability = if i % 2 == 0 {
                Durability::None
            } else {
                Durability::Immediate
            };
            let request = records(&mut buffer, 0, next, count, SEED);
            write_ok(&writer, request, durability, &mut expected)?;
            next += count as u64;
        }
        verify_exact(&db, &expected)?;
    }
    {
        let mut txn = db.begin_write()?;
        txn.restore_savepoint(&ephemeral)?;
        txn.commit()?;
    }
    verify_exact(&db, &ephemeral_contents)?;
    drop(ephemeral);
    {
        let mut txn = db.begin_write()?;
        let savepoint = txn.get_persistent_savepoint(persistent)?;
        txn.restore_savepoint(&savepoint)?;
        txn.delete_persistent_savepoint(persistent)?;
        txn.commit()?;
    }
    verify_exact(&db, &persistent_contents)?;
    let mut expected = persistent_contents.clone();

    // Savepoint methods inside closures, all within one write phase.
    let (closure_ephemeral_records, closure_persistent_records) = {
        let writer = Writer::new(&db, variant)?;
        let persistent = writer
            .write(|txn| Ok(txn.persistent_savepoint()?))
            .map_err(|e| e.to_string())?
            .value;
        let persistent_contents = expected.clone();
        for _ in 0..20 {
            let request = records(&mut buffer, 0, next, 8, SEED);
            write_ok(&writer, request, Durability::None, &mut expected)?;
            next += 8;
        }
        let ephemeral = writer
            .write(|txn| Ok(txn.ephemeral_savepoint()?))
            .map_err(|e| e.to_string())?
            .value;
        let ephemeral_contents = expected.clone();
        for _ in 0..20 {
            let request = records(&mut buffer, 0, next, 8, SEED);
            write_ok(&writer, request, Durability::Immediate, &mut expected)?;
            next += 8;
        }
        writer
            .write(|txn| Ok(txn.restore_savepoint(&ephemeral)?))
            .map_err(|e| e.to_string())?;
        verify_exact(&db, &ephemeral_contents)?;
        drop(ephemeral);
        writer
            .write(|txn| {
                let savepoint = txn.get_persistent_savepoint(persistent)?;
                txn.restore_savepoint(&savepoint)?;
                txn.delete_persistent_savepoint(persistent)?;
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        verify_exact(&db, &persistent_contents)?;
        expected = persistent_contents;
        (ephemeral_contents.len(), expected.len())
    };
    let remaining = db.begin_write()?;
    ensure!(
        remaining.list_persistent_savepoints()?.next().is_none(),
        "persistent savepoint not deleted"
    );
    remaining.abort()?;
    drop(db);
    reopen_exact(path, &expected)?;
    Ok(
        json!({"ephemeral_restore_records": ephemeral_contents.len(),
              "persistent_restore_records": persistent_contents.len(),
              "closure_ephemeral_restore_records": closure_ephemeral_records,
              "closure_persistent_restore_records": closure_persistent_records, "reopened_exact": true}),
    )
}

/// Closure `Err` aborts the whole transaction (including tables it created and
/// errors propagated from redb calls with `?`) and releases the lock; a nested
/// submission from inside a closure is refused as `NotExecuted`.
fn closure_error(db: Database, path: &PathBuf, variant: Variant) -> TestResult<Value> {
    const ROUNDS: u64 = 20;
    let mut expected = BTreeMap::new();
    let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
    let mut next = 0;
    let mut checks = Vec::new();
    {
        let writer = Writer::new(&db, variant)?;
        let mut last = write_ok(
            &writer,
            records(&mut buffer, 0, next, 8, SEED),
            Durability::Immediate,
            &mut expected,
        )?;
        next += 8;
        for round in 0..ROUNDS {
            let durability = if round % 2 == 0 {
                Durability::Immediate
            } else {
                Durability::None
            };
            let doomed: Vec<_> = records(&mut buffer, 1, round * 64, 64, SEED).to_vec();
            // Explicit Err after inserts into an existing and a newly created table.
            let failing = with_deadline("failing closure", || {
                writer.write(|txn| {
                    txn.set_durability(durability)?;
                    insert_body(&doomed, durability)(txn)?;
                    txn.open_table(SCRATCH)?.insert(round, round)?;
                    Err::<(), _>(WriteError::Closure(format!("round {round}")))
                })
            })?;
            ensure!(
                matches!(&failing, Err(WriteError::Closure(r)) if *r == format!("round {round}")),
                "round {round}: closure error not returned: {failing:?}"
            );
            // A redb error propagated with `?` after an insert (table type mismatch).
            let mismatched = with_deadline("redb-error closure", || {
                writer.write(|txn| {
                    insert_body(&doomed[..1], durability)(txn)?;
                    txn.open_table(TableDefinition::<u64, u32>::new(TABLE.name()))?;
                    Ok(())
                })
            })?;
            ensure!(
                matches!(mismatched, Err(WriteError::Other(_))),
                "round {round}: redb error not returned"
            );
            verify_exact(&db, &expected)?;
            let fresh: Vec<_> = records(&mut buffer, 0, next, 8, SEED).to_vec();
            let id = with_deadline("writer after closure error", || {
                writer.insert(&fresh, durability)
            })?
            .map_err(|e| format!("round {round}: {e}"))?
            .transaction_id;
            expected.extend(fresh);
            next += 8;
            if let (Some(a), Some(b)) = (last, id) {
                ensure!(
                    b == a + 3,
                    "round {round}: two aborted closures should consume two IDs: {a} -> {b}"
                );
            }
            last = id;
        }
        checks.extend([
            "closure-err-aborts",
            "redb-err-aborts",
            "created-table-rolled-back",
            "lock-released-after-err",
            "aborts-consume-ids",
        ]);
        if variant.delegated() {
            // Nested submission from inside a closure is refused by the bridge;
            // the outer closure still commits its own write.
            let inner = [(key(2, 0), 7)];
            let outer = [(key(2, 1), 8)];
            let nested = with_deadline("nested submission", || {
                writer.write(|txn| {
                    let nested = writer.insert(&inner, Durability::None);
                    insert_body(&outer, Durability::Immediate)(txn)?;
                    Ok(format!("{nested:?}"))
                })
            })?
            .map_err(|e| e.to_string())?
            .value;
            ensure!(
                nested == format!("{:?}", Err::<Option<u64>, _>(WriteError::NotExecuted)),
                "nested submission not refused: {nested}"
            );
            expected.extend(outer);
            verify_exact(&db, &expected)?;
            checks.push("nested-submission-not-executed");
        }
    }
    drop(db);
    reopen_exact(path, &expected)?;
    Ok(
        json!({"rounds": ROUNDS, "checks": checks, "records": expected.len(), "reopened_exact": true,
              "integrity_check": true}),
    )
}

/// None of `records` is visible in a fresh read snapshot.
fn absent(db: &Database, records: &[(u64, u64)]) -> Result<(), String> {
    let read = db.begin_read().map_err(|e| e.to_string())?;
    let table = read.open_table(TABLE).map_err(|e| e.to_string())?;
    for &(k, _) in records {
        if table.get(k).map_err(|e| e.to_string())?.is_some() {
            return Err(format!("key {k} of a panicked transaction is visible"));
        }
    }
    Ok(())
}

/// Typed panic payload so the requester can verify it received its own panic.
#[derive(Debug, PartialEq)]
struct PanicToken {
    writer: usize,
    round: u64,
}

const PANIC_TEXT: &str = "closure-panic-test";

/// Text of a `panic!` payload (`String`, or `&'static str` when const-folded).
fn panic_text(payload: &(dyn Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

fn our_panic(payload: &(dyn Any + Send)) -> bool {
    payload.downcast_ref::<PanicToken>().is_some()
        || panic_text(payload).is_some_and(|s| s.starts_with(PANIC_TEXT))
}

/// A closure panic is re-raised on the requester (typed and string payloads),
/// the transaction is rolled back (no partial writes, integrity check clean),
/// and the lock stays usable, for every variant. FC/FC-PQ must have re-raised
/// at least one panic that happened on a combiner thread.
fn closure_panic(db: Database, path: &PathBuf, variant: Variant) -> TestResult<Value> {
    const THREADS: usize = 8;
    const MIN_ROUNDS: u64 = 30;
    const MAX_ROUNDS: u64 = 3000;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !our_panic(info.payload()) {
            default_hook(info);
        }
    }));
    let remote_panics = AtomicU64::new(0);
    let panics = AtomicU64::new(0);
    let writer = Writer::new(&db, variant)?;
    let committed = thread::scope(
        |scope| -> TestResult<Vec<(Vec<(u64, u64)>, Vec<Option<u64>>)>> {
            let handles: Vec<_> = (0..THREADS).map(|w| {
            let (writer, remote_panics, panics, writer_db) = (&writer, &remote_panics, &panics, &db);
            scope.spawn(move || -> Result<(Vec<(u64, u64)>, Vec<Option<u64>>), String> {
                let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
                let mut written = Vec::new();
                let mut ids = Vec::new();
                // Every attempt writes its own keys, so a panicked transaction that
                // was committed anyway shows up as extra keys (never overwritten).
                let mut round = 0;
                while round < MIN_ROUNDS
                    || (variant.combining() && remote_panics.load(Ordering::Relaxed) == 0 && round < MAX_ROUNDS) {
                    let durability = if round % 2 == 0 { Durability::None } else { Durability::Immediate };
                    let request: Vec<_> = records(&mut buffer, w, round * 8, 8, SEED).to_vec();
                    let requester = thread::current().id();
                    let kind = round % 3;
                    let outcome = catch_unwind(AssertUnwindSafe(|| writer.write(|txn| {
                        txn.set_durability(durability)?;
                        let mut table = txn.open_table(TABLE)?;
                        for &(k, v) in &request {
                            table.insert(k, v)?;
                        }
                        if kind == 2 {
                            return Ok(());
                        }
                        if thread::current().id() != requester {
                            remote_panics.fetch_add(1, Ordering::Relaxed);
                        }
                        if kind == 0 {
                            // Unwinds with the table handle still open.
                            std::panic::panic_any(PanicToken { writer: w, round });
                        }
                        drop(table);
                        panic!("{PANIC_TEXT} {w} {round}");
                    })));
                    match (kind, outcome) {
                        (2, Ok(Ok(written_ok))) => {
                            written.extend(request);
                            ids.push(written_ok.transaction_id);
                        }
                        (0, Err(payload)) if payload.downcast_ref::<PanicToken>()
                            == Some(&PanicToken { writer: w, round }) => {
                            panics.fetch_add(1, Ordering::Relaxed);
                            absent(writer_db, &request)?;
                        }
                        (1, Err(payload)) if panic_text(payload.as_ref())
                            == Some(format!("{PANIC_TEXT} {w} {round}").as_str()) => {
                            panics.fetch_add(1, Ordering::Relaxed);
                            absent(writer_db, &request)?;
                        }
                        (kind, Err(payload)) => {
                            return Err(format!("writer {w} round {round} kind {kind}: wrong panic payload \
                                                (ours: {})", our_panic(payload.as_ref())));
                        }
                        (kind, Ok(result)) => {
                            return Err(format!("writer {w} round {round} kind {kind}: unexpected {result:?}"));
                        }
                    }
                    round += 1;
                }
                Ok((written, ids))
            })
        }).collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .map_err(|_| "requester thread panicked".into())
                        .and_then(|r| r.map_err(Into::into))
                })
                .collect()
        },
    )?;
    let mut expected = BTreeMap::new();
    for (written, ids) in &committed {
        expected.extend(written.iter().copied());
        ensure_increasing(ids)?;
    }
    // No partial writes from any panicked transaction; the lock is still usable.
    verify_exact(&db, &expected)?;
    let after = [(key(15, 0), value(SEED, key(15, 0)))];
    with_deadline("writer after panics", || {
        writer.insert(&after, Durability::Immediate)
    })?
    .map_err(|e| e.to_string())?;
    expected.extend(after);
    verify_exact(&db, &expected)?;
    let remote = remote_panics.load(Ordering::Relaxed);
    check_remote(variant, remote, "panicking closure")?;
    hook_checks(variant)?;
    drop(writer);
    drop(db);
    let _ = std::panic::take_hook();
    reopen_exact(path, &expected)?;
    Ok(
        json!({"threads": THREADS, "panics_reraised_on_requester": panics.load(Ordering::Relaxed),
              "panics_on_combiner": remote, "records": expected.len(), "reopened_exact": true,
              "integrity_check": true}),
    )
}

/// Reads inside a closure see the transaction's own uncommitted writes
/// (insert, update, remove, a second table, a reopened table handle), the
/// closure's value reaches the requester, and a later closure reads the
/// committed result; tables can be created and deleted inside closures.
fn read_own_writes(db: Database, path: &PathBuf, variant: Variant) -> TestResult<Value> {
    const OTHER: TableDefinition<&str, u64> = TableDefinition::new("closure_other");
    let mut expected = BTreeMap::new();
    let keys: Vec<u64> = (0..8).map(|i| key(0, i)).collect();
    {
        let writer = Writer::new(&db, variant)?;
        let observed = writer
            .write(|txn| {
                txn.set_durability(Durability::Immediate)?;
                txn.set_two_phase_commit(true);
                txn.set_quick_repair(true);
                let mut observed = Vec::new();
                {
                    let mut table = txn.open_table(TABLE)?;
                    for &k in &keys {
                        table.insert(k, value(SEED, k))?;
                    }
                    for &k in &keys {
                        observed.push(table.get(k)?.map(|v| v.value()));
                    }
                    let old = table.insert(keys[0], 1)?.map(|v| v.value());
                    observed.push(old);
                    observed.push(table.get(keys[0])?.map(|v| v.value()));
                    observed.push(table.remove(keys[1])?.map(|v| v.value()));
                    observed.push(table.get(keys[1])?.map(|v| v.value()));
                    observed.push(Some(table.range(keys[0]..=keys[7])?.count() as u64));
                }
                {
                    let mut other = txn.open_table(OTHER)?;
                    other.insert("balance", 40)?;
                    let balance = other.get("balance")?.map(|v| v.value()).unwrap_or(0);
                    other.insert("balance", balance + 2)?;
                    observed.push(other.get("balance")?.map(|v| v.value()));
                }
                // A reopened handle sees the same uncommitted state.
                let table = txn.open_table(TABLE)?;
                observed.push(table.get(keys[0])?.map(|v| v.value()));
                observed.push(Some(table.len()?));
                Ok(observed)
            })
            .map_err(|e| e.to_string())?
            .value;
        let mut want: Vec<Option<u64>> = keys.iter().map(|&k| Some(value(SEED, k))).collect();
        want.extend([
            Some(value(SEED, keys[0])),
            Some(1),
            Some(value(SEED, keys[1])),
            None,
            Some(7),
            Some(42),
            Some(1),
            Some(7),
        ]);
        ensure!(
            observed == want,
            "in-transaction reads differ: {observed:?} != {want:?}"
        );
        for &k in &keys[2..] {
            expected.insert(k, value(SEED, k));
        }
        expected.insert(keys[0], 1);
        verify_exact(&db, &expected)?;
        // A later closure reads the committed state (read-modify-write).
        let (first, balance) = writer
            .write(|txn| {
                let mut table = txn.open_table(TABLE)?;
                let first = table.get(keys[0])?.map(|v| v.value());
                table.insert(keys[0], first.unwrap_or(0) + 1)?;
                let other = txn.open_table(OTHER)?;
                let balance = other.get("balance")?.map(|v| v.value());
                Ok((first, balance))
            })
            .map_err(|e| e.to_string())?
            .value;
        ensure!(
            (first, balance) == (Some(1), Some(42)),
            "committed state not visible: {first:?}, {balance:?}"
        );
        expected.insert(keys[0], 2);
        // Delete a table inside a closure.
        let deleted = writer
            .write(|txn| Ok(txn.delete_table(OTHER)?))
            .map_err(|e| e.to_string())?
            .value;
        ensure!(deleted, "delete_table inside a closure found no table");
        verify_exact(&db, &expected)?;
    }
    let read = db.begin_read()?;
    ensure!(
        read.open_table(OTHER).is_err(),
        "deleted table still readable"
    );
    drop(read);
    drop(db);
    reopen_exact(path, &expected)?;
    Ok(
        json!({"in_transaction_reads": "exact", "committed_reads": "exact", "table_create_delete": true,
              "reopened_exact": true}),
    )
}

/// Transfer stress: 16 writers + 2 readers. Every committed transfer also
/// appends a log row (a closure run twice would find its row and fail). Every
/// reader snapshot conserves the total; the final and reopened states conserve
/// it and, for ID-exposing variants, equal a replay of the committed transfers
/// in transaction-ID order; aborted transfers consume IDs.
fn transfer_stress(
    db: Database,
    path: &PathBuf,
    variant: Variant,
    durability: Durability,
) -> TestResult<Value> {
    const WRITERS: usize = 16;
    const READERS: usize = 2;
    let per_writer: u64 = if matches!(durability, Durability::Immediate) {
        150
    } else {
        500
    };
    seed_accounts(&db, STRESS_ACCOUNTS)?;
    let done = AtomicBool::new(false);
    let snapshots = AtomicU64::new(0);
    let remote = AtomicU64::new(0);
    let writer = Writer::new(&db, variant)?;
    let first_marker = marker(&writer)?;
    type Committed = Vec<(Option<u64>, u64, u64, u64)>;
    let (per_worker, reader_errors) = thread::scope(|scope| {
        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                let (db, done, snapshots) = (&db, &done, &snapshots);
                scope.spawn(move || -> Result<(), String> {
                    let mut last_log = 0;
                    while !done.load(Ordering::Acquire) {
                        conserved_balances(db, STRESS_ACCOUNTS)
                            .map_err(|e| format!("reader snapshot: {e}"))?;
                        let read = db.begin_read().map_err(|e| e.to_string())?;
                        if let Ok(log) = read.open_table(TRANSFER_LOG) {
                            let len = log.len().map_err(|e| e.to_string())?;
                            if len < last_log {
                                return Err(format!(
                                    "transfer log shrank from {last_log} to {len}"
                                ));
                            }
                            last_log = len;
                        }
                        if !done.load(Ordering::Acquire) {
                            snapshots.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        let writers: Vec<_> = (0..WRITERS)
            .map(|w| {
                let (writer, remote) = (&writer, &remote);
                scope.spawn(move || -> Result<(Committed, u64), String> {
                    let mut transfers = Transfers::new(SEED, w, STRESS_ACCOUNTS);
                    let mut committed = Vec::new();
                    let mut aborted = 0;
                    for sequence in 0..per_writer {
                        let (from, to, amount) = transfers.next_transfer();
                        let body = transfer_body(from, to, amount, durability);
                        let outcome = writer.write(probed(remote, move |txn| {
                            body(txn)?;
                            if txn
                                .open_table(TRANSFER_LOG)?
                                .insert(key(w, sequence), amount)?
                                .is_some()
                            {
                                return Err(WriteError::Other(format!(
                                    "transfer {w}/{sequence} ran twice"
                                )));
                            }
                            Ok(())
                        }));
                        match outcome {
                            Ok(written) => {
                                committed.push((written.transaction_id, from, to, amount))
                            }
                            Err(WriteError::Insufficient {
                                account,
                                balance,
                                amount: a,
                            }) if account == from && a == amount && balance < amount => {
                                aborted += 1
                            }
                            Err(error) => return Err(format!("writer {w}: {error}")),
                        }
                    }
                    Ok((committed, aborted))
                })
            })
            .collect();
        let results: Vec<_> = writers
            .into_iter()
            .map(|h| h.join().unwrap_or(Err("writer panic".into())))
            .collect();
        done.store(true, Ordering::Release);
        let reader_errors: Vec<_> = readers
            .into_iter()
            .filter_map(|h| h.join().unwrap_or(Err("reader panic".into())).err())
            .collect();
        (results, reader_errors)
    });
    ensure!(
        reader_errors.is_empty(),
        "reader failures: {reader_errors:?}"
    );
    let per_worker = per_worker.into_iter().collect::<Result<Vec<_>, _>>()?;
    let last_marker = marker(&writer)?;
    let committed: Vec<_> = per_worker
        .iter()
        .flat_map(|(c, _)| c.iter().copied())
        .collect();
    let aborted: u64 = per_worker.iter().map(|(_, a)| a).sum();
    let total = WRITERS as u64 * per_writer;
    ensure!(committed.len() as u64 + aborted == total, "lost transfers");
    ensure!(aborted > 0, "no insufficient-balance abort exercised");
    let balances = conserved_balances(&db, STRESS_ACCOUNTS)?;
    // The log holds exactly the committed transfers.
    let (log_rows, log_sum) = {
        let read = db.begin_read()?;
        let log = read.open_table(TRANSFER_LOG)?;
        let mut sum = 0;
        for pair in log.iter()? {
            sum += pair?.1.value();
        }
        (log.len()?, sum)
    };
    let committed_sum: u64 = committed.iter().map(|&(_, _, _, amount)| amount).sum();
    ensure!(
        log_rows == committed.len() as u64 && log_sum == committed_sum,
        "transfer log ({log_rows} rows, sum {log_sum}) != committed ({}, sum {committed_sum})",
        committed.len()
    );
    let snapshot_count = snapshots.load(Ordering::Relaxed);
    ensure!(
        snapshot_count > 0,
        "no reader snapshot completed while writers were active"
    );
    let mut id_check = "not exposed by upstream redb";
    if let (Some(first), Some(last)) = (first_marker, last_marker) {
        for (worker, _) in &per_worker {
            ensure_increasing(&worker.iter().map(|c| c.0).collect::<Vec<_>>())?;
        }
        // Committed and aborted transfers together consume every ID in between.
        ensure!(
            last - first - 1 == total,
            "IDs between markers: {} for {total} transfers",
            last - first - 1
        );
        let mut ordered: Vec<_> = committed
            .iter()
            .map(|&(id, from, to, amount)| (id.unwrap(), from, to, amount))
            .collect();
        ordered.sort_unstable();
        ensure!(
            ordered.windows(2).all(|p| p[0].0 < p[1].0)
                && ordered.iter().all(|c| c.0 > first && c.0 < last),
            "committed transfer IDs not unique within the phase"
        );
        let mut replay = vec![INITIAL_BALANCE; STRESS_ACCOUNTS as usize];
        for &(id, from, to, amount) in &ordered {
            ensure!(
                replay[from as usize] >= amount,
                "replay: transfer {id} overdraws account {from}"
            );
            replay[from as usize] -= amount;
            replay[to as usize] += amount;
        }
        ensure!(
            replay == balances,
            "final balances differ from the ID-ordered replay"
        );
        id_check =
            "per-writer increasing; committed+aborted fill the ID range; ID-order replay exact";
    }
    let remote_closures = remote.load(Ordering::Relaxed);
    check_remote(variant, remote_closures, "closure")?;
    hook_checks(variant)?;
    drop(writer);
    drop(db);
    let mut reopened = Database::open(path)?;
    ensure!(
        conserved_balances(&reopened, STRESS_ACCOUNTS)? == balances,
        "reopened balances differ"
    );
    ensure!(
        reopened.begin_read()?.open_table(TRANSFER_LOG)?.len()? == log_rows,
        "reopened log differs"
    );
    ensure!(
        reopened.check_integrity()?,
        "integrity check after reopen had to repair the database"
    );
    Ok(
        json!({"writers": WRITERS, "readers": READERS, "durability": format!("{durability:?}"),
              "transfers": total, "committed": committed.len(), "aborted_insufficient": aborted,
              "reader_snapshots_conserved": snapshot_count, "transaction_ids": id_check,
              "closures_run_on_other_thread": remote_closures, "bodies_run_on_other_thread": remote_executions(),
              "max_body_occupancy": occupancy(), "reopened_identical": true, "integrity_check": true}),
    )
}

#[cfg(feature = "test_hooks")]
mod hooks {
    use super::*;
    use redb::dlock_private::test_hooks;

    /// A body paused inside the lock (closure done, not committed): reads proceed
    /// and see the last committed snapshot; a second writer stays blocked.
    pub fn paused_writer(db: Database, variant: Variant) -> TestResult<Value> {
        let mut expected = BTreeMap::new();
        let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
        let writer = Writer::new(&db, variant)?;
        write_ok(
            &writer,
            records(&mut buffer, 0, 0, 64, SEED),
            Durability::Immediate,
            &mut expected,
        )?;
        let first: Vec<_> = records(&mut buffer, 1, 0, 64, SEED).to_vec();
        let second: Vec<_> = records(&mut buffer, 2, 0, 8, SEED).to_vec();
        let second_done = AtomicBool::new(false);
        let reads = thread::scope(|scope| -> TestResult<u32> {
            test_hooks::arm_pause();
            let paused = scope.spawn(|| writer.insert(&first, Durability::Immediate));
            let start = Instant::now();
            while !test_hooks::is_paused() {
                ensure!(
                    start.elapsed() < DEADLINE,
                    "writer never reached the pause point"
                );
                thread::yield_now();
            }
            let blocked = scope.spawn(|| {
                let outcome = writer.insert(&second, Durability::None);
                second_done.store(true, Ordering::Release);
                outcome
            });
            let mut reads = 0;
            let read_start = Instant::now();
            while read_start.elapsed() < Duration::from_millis(300) {
                verify_exact(&db, &expected)?; // uncommitted inserts invisible, no blocking
                reads += 1;
            }
            ensure!(test_hooks::is_paused(), "writer left the pause early");
            ensure!(
                !second_done.load(Ordering::Acquire),
                "second writer ran while the first held the lock"
            );
            test_hooks::release_pause();
            paused
                .join()
                .map_err(|_| "paused writer panicked")?
                .map_err(|e| e.to_string())?;
            blocked
                .join()
                .map_err(|_| "blocked writer panicked")?
                .map_err(|e| e.to_string())?;
            Ok(reads)
        })?;
        expected.extend(first);
        expected.extend(second);
        verify_exact(&db, &expected)?;
        ensure!(
            test_hooks::max_occupancy() == 1,
            "max occupancy {}",
            test_hooks::max_occupancy()
        );
        Ok(json!({"reads_while_writer_paused": reads, "second_writer_blocked": true}))
    }

    /// A panic inside a delegated body but outside the closure (injected before
    /// commit) must abort the process (checked by the gate as SIGABRT);
    /// returning here is a failure.
    pub fn body_panic(db: Database, variant: Variant) -> TestResult<Value> {
        ensure!(
            variant.delegated(),
            "body-panic fail-stop applies to delegated variants only"
        );
        let writer = Writer::new(&db, variant)?;
        test_hooks::panic_next();
        let request = [(key(0, 0), 1)];
        let outcome = thread::scope(|scope| {
            scope
                .spawn(|| writer.insert(&request, Durability::None))
                .join()
        });
        Err(format!("panic inside the delegated body did not abort: {outcome:?}").into())
    }
}
