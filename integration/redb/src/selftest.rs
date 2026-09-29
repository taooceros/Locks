//! Real-database self-tests for the correctness gate (`correctness.py`).
//! Each case uses a fresh database file and prints one JSON object on success.
use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
#[cfg(feature = "patched")]
use std::time::Instant;

use redb::{Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata};
use serde_json::{json, Value};

use crate::writer::{Variant, WriteError, Writer, Written, MAX_RECORDS_PER_REQUEST, TABLE};
use crate::{create_database, key, option, parse_durability, records, value};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const SEED: u64 = 0x5EED_0F_7E57;
const DEADLINE: Duration = Duration::from_secs(20);

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
    let details = match case.as_str() {
        "contents" => contents(db, &path, variant)?,
        "stress" => stress(db, &path, variant, parse_durability(&option("--durability", args)?)?)?,
        "errors" => errors(db, &path, variant)?,
        "savepoints" => savepoints(db, &path, variant)?,
        #[cfg(feature = "test_hooks")]
        "paused-writer" => hooks::paused_writer(db, variant)?,
        #[cfg(feature = "test_hooks")]
        "injected-error" => hooks::injected_error(db, &path, variant)?,
        #[cfg(feature = "test_hooks")]
        "panic" => hooks::panic(db, variant)?,
        _ => return Err(format!("unknown self-test for this binary: {case}").into()),
    };
    println!("{}", json!({"self_test": case, "variant": variant.name(), "status": "ok",
                          "test_hooks": cfg!(feature = "test_hooks"), "details": details}));
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
            Some((&ek, &ev)) => return Err(format!("content mismatch: read {k}={v}, expected {ek}={ev}").into()),
            None => return Err(format!("unexpected extra key {k}").into()),
        }
    }
    ensure!(expected_iter.next().is_none(), "missing keys: table has {} of {}", table.len()?, expected.len());
    Ok(())
}

fn reopen_exact(path: &PathBuf, expected: &BTreeMap<u64, u64>) -> TestResult {
    let reopened = Database::open(path)?;
    verify_exact(&reopened, expected)
}

fn write_ok(writer: &Writer<'_>, request: &[(u64, u64)], durability: Durability,
            expected: &mut BTreeMap<u64, u64>) -> TestResult<Option<u64>> {
    let written = writer.write(request, durability).map_err(|e| e.to_string())?;
    expected.extend(request.iter().copied());
    Ok(written.transaction_id)
}

/// Transaction IDs must be strictly increasing in submission order.
fn ensure_increasing(ids: &[Option<u64>]) -> TestResult {
    let ids: Vec<u64> = ids.iter().flatten().copied().collect();
    ensure!(ids.windows(2).all(|pair| pair[0] < pair[1]), "transaction IDs not increasing: {ids:?}");
    Ok(())
}

/// Run `work` on another thread; fail instead of hanging if the lock leaked.
/// The thread is joined explicitly (pthread_join) on success, so per-thread
/// lock state (U-SCL's pthread TLS) is torn down before the lock can be dropped.
fn with_deadline<T: Send>(label: &str, work: impl FnOnce() -> T + Send) -> TestResult<T> {
    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let handle = scope.spawn(move || { let _ = sender.send(work()); });
        let outcome = receiver.recv_timeout(DEADLINE)
            .map_err(|_| format!("{label}: no progress within {DEADLINE:?}; lock not released?"))?;
        handle.join().map_err(|_| format!("{label}: worker panicked"))?;
        Ok(outcome)
    })
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
            let durability = if (i / 3) % 2 == 0 { Durability::Immediate } else { Durability::None };
            let request = records(&mut buffer, 0, next, count, SEED);
            ids.push(write_ok(&writer, request, durability, &mut expected)?);
            next += count as u64;
            verify_exact(&db, &expected)?;
        }
        // A lone requester never finds another request pending, so every FC-PQ
        // request must take the E0(b) fast path when it is compiled in.
        fast_path_hits = writer.fast_path_hits();
        if variant == Variant::FcPq && cfg!(feature = "fcpq_fast_path") {
            ensure!(fast_path_hits.is_some() || !cfg!(feature = "fcpq_fast_path_stat"),
                    "fast-path counter missing");
            if let Some(hits) = fast_path_hits {
                ensure!(hits == 90, "lone FC-PQ requester took the fast path {hits}/90 times");
            }
        }
    }
    ensure_increasing(&ids)?;
    if variant != Variant::Native {
        let ids: Vec<u64> = ids.iter().flatten().copied().collect();
        ensure!(ids.windows(2).all(|p| p[1] == p[0] + 1), "IDs not contiguous without aborts: {ids:?}");
    }
    drop(db);
    reopen_exact(path, &expected)?;
    Ok(json!({"requests": 90, "records": expected.len(), "reopened_exact": true,
              "fast_path_hits": fast_path_hits}))
}

fn stress(db: Database, path: &PathBuf, variant: Variant, durability: Durability) -> TestResult<Value> {
    const WRITERS: usize = 16;
    const READERS: usize = 2;
    let per_writer: u64 = if matches!(durability, Durability::Immediate) { 200 } else { 600 };
    let done = AtomicBool::new(false);
    let reads_during_writes = AtomicU64::new(0);
    let writer = Writer::new(&db, variant)?;
    let wall_begin = crate::tsc();
    let (worker_ids, reader_errors) = thread::scope(|scope| {
        let readers: Vec<_> = (0..READERS).map(|r| {
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
                    for pair in table.range(key(owner, 0)..key(owner + 1, 0)).map_err(|e| e.to_string())? {
                        let (k, v) = pair.map_err(|e| e.to_string())?;
                        if k.value() != key(owner, expect) || v.value() != value(SEED, k.value()) {
                            return Err(format!("snapshot of writer {owner} not a committed prefix at {expect}"));
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
        }).collect();
        let writers: Vec<_> = (0..WRITERS).map(|w| {
            let writer = &writer;
            scope.spawn(move || -> Result<Vec<Written>, String> {
                let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
                let mut next = 0;
                let mut written = Vec::new();
                for i in 0..per_writer as usize {
                    let count = [1, 8, 64][(w + i) % 3];
                    let request = records(&mut buffer, w, next, count, SEED);
                    written.push(writer.write(request, durability).map_err(|e| format!("writer {w}: {e}"))?);
                    next += count as u64;
                }
                Ok(written)
            })
        }).collect();
        let ids: Vec<_> = writers.into_iter().map(|h| h.join().unwrap_or(Err("writer panic".into()))).collect();
        done.store(true, Ordering::Release);
        let reader_errors: Vec<_> = readers.into_iter()
            .filter_map(|h| h.join().unwrap_or(Err("reader panic".into())).err()).collect();
        (ids, reader_errors)
    });
    let wall_tsc = crate::tsc() - wall_begin;
    ensure!(reader_errors.is_empty(), "reader failures: {reader_errors:?}");
    let written = worker_ids.into_iter().collect::<Result<Vec<_>, _>>()?;
    let worker_ids: Vec<Vec<Option<u64>>> = written.iter()
        .map(|w| w.iter().map(|x| x.transaction_id).collect()).collect();
    // Service time: every body is charged to its requester; bodies are
    // serialised, so their total cannot exceed the phase's wall time, and it
    // must equal the total seen on the executing threads (test hooks).
    let service: Vec<u64> = written.iter().map(|w| w.iter().map(|x| x.service_tsc).sum()).collect();
    let service_total: u64 = service.iter().sum();
    if cfg!(feature = "service_time") {
        ensure!(written.iter().flatten().all(|x| x.service_tsc > 0), "a committed body was charged no service time");
        ensure!(service_total <= wall_tsc,
                "charged service {service_total} exceeds wall {wall_tsc} ticks: bodies overlapped or were double-charged");
    }
    #[cfg(feature = "test_hooks")]
    ensure!(service_total == redb::dlock_private::test_hooks::service_tsc_total(),
            "service charged to requesters {service_total} != executed {}",
            redb::dlock_private::test_hooks::service_tsc_total());
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
    ensure!(reads > 0, "no read transaction completed while writers were active");
    let mut id_check = "not exposed by upstream redb";
    if variant != Variant::Native {
        for ids in &worker_ids {
            ensure_increasing(ids)?;
        }
        let mut all: Vec<u64> = worker_ids.iter().flatten().flatten().copied().collect();
        all.sort_unstable();
        ensure!(all.len() == WRITERS * per_writer as usize, "missing transaction IDs");
        ensure!(all.windows(2).all(|p| p[1] == p[0] + 1), "transaction IDs not unique and contiguous");
        id_check = "unique, contiguous, per-writer increasing";
    }
    #[cfg(feature = "test_hooks")]
    ensure!(redb::dlock_private::test_hooks::max_occupancy() == 1,
            "max writers observed inside the body: {}", redb::dlock_private::test_hooks::max_occupancy());
    let remote = remote_executions();
    #[cfg(feature = "test_hooks")]
    match variant {
        Variant::Fc | Variant::FcPq => ensure!(remote.as_u64() > Some(0), "no body ran on a combiner"),
        Variant::Refactored | Variant::BridgeMutex | Variant::Mcs | Variant::Uscl =>
            ensure!(remote.as_u64() == Some(0), "a requester-run variant ran a body on another thread"),
        Variant::Native => unreachable!("native has no test-hook binary"),
    }
    drop(writer);
    drop(db);
    reopen_exact(path, &expected)?;
    Ok(json!({"writers": WRITERS, "bodies_run_on_other_thread": remote, "transactions": WRITERS as u64 * per_writer,
              "service_tsc_per_writer": service, "service_over_wall": service_total as f64 / wall_tsc as f64,
              "records": expected.len(), "durability": format!("{durability:?}"),
              "reads_during_writes": reads, "transaction_ids": id_check,
              "max_body_occupancy": occupancy(), "reopened_exact": true}))
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
        let mut write = |count: usize, expected: &mut BTreeMap<u64, u64>| -> TestResult<Option<u64>> {
            let request = records(&mut buffer, 0, next, count, SEED);
            next += count as u64;
            write_ok(&writer, request, Durability::Immediate, expected)
        };
        let first = write(1, &mut expected)?;
        // Shape rejections happen before submission: no lock, no transaction ID.
        let oversized: Vec<(u64, u64)> = (0..=MAX_RECORDS_PER_REQUEST as u64)
            .map(|i| (key(1, i), value(SEED, key(1, i)))).collect();
        ensure!(matches!(writer.write(&[], Durability::Immediate), Err(WriteError::Rejected(_))), "empty request accepted");
        ensure!(matches!(writer.write(&oversized, Durability::None), Err(WriteError::Rejected(_))), "65-record request accepted");
        let second = write(8, &mut expected)?;
        if let (Some(a), Some(b)) = (first, second) {
            ensure!(b == a + 1, "rejected requests consumed transaction IDs: {a} -> {b}");
        }
        verify_exact(&db, &expected)?;
        checks.push("rejections");

        // Duplicate key after fresh inserts: the whole transaction is aborted.
        let existing = *expected.keys().next().expect("seeded");
        let partial = [(key(2, 0), 1), (key(2, 1), 2), (existing, 3)];
        ensure!(matches!(writer.write(&partial, Durability::Immediate), Err(WriteError::Duplicate(k)) if k == existing),
                "duplicate key not reported");
        ensure!(matches!(writer.write(&[(key(2, 5), 1), (key(2, 5), 2)], Durability::None), Err(WriteError::Duplicate(_))),
                "in-request duplicate not reported");
        verify_exact(&db, &expected)?;
        let third = write(1, &mut expected)?;
        if let (Some(b), Some(c)) = (second, third) {
            // Aborted transactions consume IDs exactly as upstream redb's tracker does.
            ensure!(c == b + 3, "aborted transactions changed ID order: {b} -> {c}");
        }
        checks.push("duplicate-abort");

        // Abort releases the lock: alternate aborting and committing threads.
        for round in 0..50_u64 {
            let dup = [(existing, round)];
            let outcome = with_deadline("aborting writer", || writer.write(&dup, Durability::None))?;
            ensure!(matches!(outcome, Err(WriteError::Duplicate(_))), "round {round}: duplicate not reported");
            let fresh = [(key(3, round), value(SEED, key(3, round)))];
            with_deadline("writer after abort", || writer.write(&fresh, Durability::None))?
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

#[cfg(feature = "native")]
fn errors_gated(_: &Database, _: &mut BTreeMap<u64, u64>) -> TestResult {
    unreachable!("native has no gate")
}

#[cfg(feature = "patched")]
fn errors_gated(db: &Database, expected: &mut BTreeMap<u64, u64>) -> TestResult {
    use redb::dlock_private::{DelegatedWriteGate, FixedInsert, FixedInsertTarget};
    let target = FixedInsertTarget::new(db, TABLE)?;
    let tracker_write = |k: u64| -> TestResult<u64> {
        Ok(target.execute_native(&FixedInsert { records: &[(k, value(SEED, k))], durability: Durability::Immediate })
            .outcome?)
    };
    let before = tracker_write(key(4, 0))?;
    expected.insert(key(4, 0), value(SEED, key(4, 0)));
    let gate = DelegatedWriteGate::enter(target)?;
    ensure!(DelegatedWriteGate::enter(target).is_err(), "second gate on one database accepted");
    let mut delegated = Vec::new();
    for i in 1..4 {
        let k = key(4, i);
        delegated.push(gate.execute(&FixedInsert { records: &[(k, value(SEED, k))], durability: Durability::None },
                                    |call| call.run()).outcome?);
        expected.insert(k, value(SEED, k));
    }
    ensure!(delegated[0] == before + 1 && delegated.windows(2).all(|p| p[1] == p[0] + 1),
            "IDs across gate entry not continuous: {before} then {delegated:?}");
    // A public writer waits on the tracker's slot while the gate holds it.
    let public_done = AtomicBool::new(false);
    thread::scope(|scope| -> TestResult {
        let public = scope.spawn(|| -> Result<(), String> {
            let txn = db.begin_write().map_err(|e| e.to_string())?;
            {
                let mut table = txn.open_table(TABLE).map_err(|e| e.to_string())?;
                table.insert(key(5, 0), value(SEED, key(5, 0))).map_err(|e| e.to_string())?;
            }
            txn.commit().map_err(|e| e.to_string())?;
            public_done.store(true, Ordering::Release);
            Ok(())
        });
        thread::sleep(Duration::from_millis(300));
        ensure!(!public_done.load(Ordering::Acquire), "public begin_write ran while the gate was held");
        drop(gate);
        let start = Instant::now();
        while !public.is_finished() {
            ensure!(start.elapsed() < DEADLINE, "public writer not admitted after the gate was dropped");
            thread::sleep(Duration::from_millis(5));
        }
        public.join().map_err(|_| "public writer panicked")??;
        Ok(())
    })?;
    expected.insert(key(5, 0), value(SEED, key(5, 0)));
    let after = tracker_write(key(4, 9))?;
    expected.insert(key(4, 9), value(SEED, key(4, 9)));
    // last delegated ID, then the public writer, then this tracker writer.
    ensure!(after == delegated[2] + 2, "IDs across gate exit not continuous: {delegated:?} then {after}");
    verify_exact(db, expected)
}

/// Persistent and ephemeral savepoints taken before a write phase remain valid
/// across it (delegated writes for bridge variants) and restore exactly.
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
            let durability = if i % 2 == 0 { Durability::None } else { Durability::Immediate };
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
    drop(db);
    reopen_exact(path, &persistent_contents)?;
    Ok(json!({"ephemeral_restore_records": ephemeral_contents.len(),
              "persistent_restore_records": persistent_contents.len(), "reopened_exact": true}))
}

#[cfg(feature = "test_hooks")]
mod hooks {
    use super::*;
    use redb::dlock_private::test_hooks;

    /// A body paused inside the lock (inserts done, not committed): reads proceed
    /// and see the last committed snapshot; a second writer stays blocked.
    pub fn paused_writer(db: Database, variant: Variant) -> TestResult<Value> {
        let mut expected = BTreeMap::new();
        let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
        let writer = Writer::new(&db, variant)?;
        write_ok(&writer, records(&mut buffer, 0, 0, 64, SEED), Durability::Immediate, &mut expected)?;
        let first: Vec<_> = records(&mut buffer, 1, 0, 64, SEED).to_vec();
        let second: Vec<_> = records(&mut buffer, 2, 0, 8, SEED).to_vec();
        let second_done = AtomicBool::new(false);
        let reads = thread::scope(|scope| -> TestResult<u32> {
            test_hooks::arm_pause();
            let paused = scope.spawn(|| writer.write(&first, Durability::Immediate));
            let start = Instant::now();
            while !test_hooks::is_paused() {
                ensure!(start.elapsed() < DEADLINE, "writer never reached the pause point");
                thread::yield_now();
            }
            let blocked = scope.spawn(|| {
                let outcome = writer.write(&second, Durability::None);
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
            ensure!(!second_done.load(Ordering::Acquire), "second writer ran while the first held the lock");
            test_hooks::release_pause();
            paused.join().map_err(|_| "paused writer panicked")?.map_err(|e| e.to_string())?;
            blocked.join().map_err(|_| "blocked writer panicked")?.map_err(|e| e.to_string())?;
            Ok(reads)
        })?;
        expected.extend(first);
        expected.extend(second);
        verify_exact(&db, &expected)?;
        ensure!(test_hooks::max_occupancy() == 1, "max occupancy {}", test_hooks::max_occupancy());
        Ok(json!({"reads_while_writer_paused": reads, "second_writer_blocked": true}))
    }

    /// An error inside the body after inserts: aborted, contents unchanged, lock released.
    pub fn injected_error(db: Database, path: &PathBuf, variant: Variant) -> TestResult<Value> {
        let mut expected = BTreeMap::new();
        let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
        let mut next = 0;
        {
            let writer = Writer::new(&db, variant)?;
            for round in 0..20 {
                test_hooks::fail_next();
                let failing: Vec<_> = records(&mut buffer, 1, round * 64, 64, SEED).to_vec();
                let outcome = with_deadline("failing writer", || writer.write(&failing, Durability::Immediate))?;
                ensure!(matches!(outcome, Err(WriteError::Injected)), "round {round}: injected error not reported");
                verify_exact(&db, &expected)?;
                let fresh: Vec<_> = records(&mut buffer, 0, next, 8, SEED).to_vec();
                with_deadline("writer after error", || writer.write(&fresh, Durability::None))?
                    .map_err(|e| format!("round {round}: {e}"))?;
                expected.extend(fresh);
                next += 8;
            }
        }
        drop(db);
        reopen_exact(path, &expected)?;
        Ok(json!({"rounds": 20, "records": expected.len(), "reopened_exact": true}))
    }

    /// A panic inside a delegated body must abort the process (checked by the
    /// gate as SIGABRT); returning here is a failure.
    pub fn panic(db: Database, variant: Variant) -> TestResult<Value> {
        ensure!(variant.delegated(), "panic fail-stop applies to delegated variants only");
        let writer = Writer::new(&db, variant)?;
        test_hooks::panic_next();
        let request = [(key(0, 0), 1)];
        let outcome = thread::scope(|scope| scope.spawn(|| writer.write(&request, Durability::None)).join());
        Err(format!("panic inside the delegated body did not abort: {outcome:?}").into())
    }
}
