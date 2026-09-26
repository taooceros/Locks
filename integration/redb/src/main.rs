//! Eight saturated, pinned requesters issuing fixed-shape redb write requests
//! through one of six variants (see writer.rs). Read transactions
//! (`Database::begin_read`) never pass through any write lock or bridge.
#[cfg(all(feature = "native", feature = "patched"))]
compile_error!("build exactly one of the `native` and `patched` features");
#[cfg(not(any(feature = "native", feature = "patched")))]
compile_error!("build exactly one of the `native` and `patched` features");

#[cfg(feature = "native")]
extern crate redb_upstream as redb;

mod selftest;
mod writer;

use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{Barrier, OnceLock},
    thread,
    time::{Duration, Instant},
};

use redb::{Database, Durability, ReadableDatabase, ReadableTable};
use serde::Serialize;

use writer::{Variant, Writer, MAX_RECORDS_PER_REQUEST, TABLE};

const WORKERS: usize = 8;
const MAX_RECORDS: u64 = 64_000_000;
const MAX_RECORDS_PER_WORKER: u64 = MAX_RECORDS / WORKERS as u64;
const WINDOW_MS: u64 = 250;
const WINDOWS: usize = 8;

pub fn value(seed: u64, key: u64) -> u64 {
    let mut v = seed ^ key.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    v ^= v >> 30;
    v = v.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    v ^= v >> 27;
    v = v.wrapping_mul(0x94d0_49bb_1331_11eb);
    v ^ (v >> 31)
}

pub fn key(worker: usize, sequence: u64) -> u64 {
    ((worker as u64) << 56) | sequence
}

/// Fill `buffer` with the next `count` records of `worker`, outside any lock.
pub fn records(buffer: &mut [(u64, u64); MAX_RECORDS_PER_REQUEST], worker: usize, first: u64,
               count: usize, seed: u64) -> &[(u64, u64)] {
    for (offset, slot) in buffer[..count].iter_mut().enumerate() {
        let k = key(worker, first + offset as u64);
        *slot = (k, value(seed, k));
    }
    &buffer[..count]
}

pub fn create_database(path: &PathBuf) -> Result<Database, Box<dyn Error + Send + Sync>> {
    if path.exists() {
        return Err(format!("database already exists: {}", path.display()).into());
    }
    fs::create_dir_all(path.parent().ok_or("database needs a parent directory")?)?;
    let db = Database::create(path)?;
    let mut txn = db.begin_write()?;
    txn.set_durability(Durability::Immediate)?;
    { let _table = txn.open_table(TABLE)?; }
    txn.commit()?; // Table creation precedes any gate and the measured window.
    Ok(db)
}

#[derive(Serialize)]
struct WorkerResult {
    cpu: usize,
    records_per_transaction: u64,
    completed_transactions: u64,
    completed_records: u64,
    window_transactions: [u64; WINDOWS],
    window_records: [u64; WINDOWS],
    #[serde(serialize_with = "serialize_histogram")]
    response_ns_log2: [u64; 64],
}

fn serialize_histogram<S: serde::Serializer>(values: &[u64; 64], serializer: S) -> Result<S::Ok, S::Error> {
    values.as_slice().serialize(serializer)
}

fn cpu_time_ns() -> Result<u64, Box<dyn Error + Send + Sync>> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((time.tv_sec as u64) * 1_000_000_000 + time.tv_nsec as u64)
}

#[allow(clippy::too_many_arguments)]
fn worker(
    id: usize, cpu: usize, batch: usize, seed: u64, durability: Durability,
    writer: &Writer<'_>, barrier: &Barrier, start: &OnceLock<Instant>,
    duration: Duration, smoke_transactions: Option<u64>,
) -> Result<WorkerResult, String> {
    let pinned = core_affinity::set_for_current(core_affinity::CoreId { id: cpu });
    barrier.wait();
    // The first rendezvous reports readiness; the second publishes the clock.
    barrier.wait();
    if !pinned {
        return Err(format!("cannot pin worker {id} to CPU {cpu}"));
    }
    let begin = *start.get().expect("main initialized clock between barriers");
    let mut output = WorkerResult {
        cpu, records_per_transaction: batch as u64,
        completed_transactions: 0, completed_records: 0,
        window_transactions: [0; WINDOWS], window_records: [0; WINDOWS],
        response_ns_log2: [0; 64],
    };
    let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
    loop {
        if let Some(limit) = smoke_transactions {
            if output.completed_transactions >= limit { break; }
        } else if begin.elapsed() >= duration { break; }
        if output.completed_records + batch as u64 > MAX_RECORDS_PER_WORKER {
            return Err(format!("worker {id} reached {MAX_RECORDS_PER_WORKER}-record cap; trial invalid"));
        }
        let request = records(&mut buffer, id, output.completed_records, batch, seed);
        let submitted = Instant::now();
        let outcome = writer.write(request, durability);
        let response_ns = submitted.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let completed = begin.elapsed();
        outcome.map_err(|error| format!("worker {id}, transaction {}: {error}", output.completed_transactions))?;
        output.completed_transactions += 1;
        output.completed_records += batch as u64;
        // Only requests completed within the fixed window are credited; a draining
        // request remains in the final database oracle.
        if completed < duration || smoke_transactions.is_some() {
            let window = ((completed.as_millis() / WINDOW_MS as u128) as usize).min(WINDOWS - 1);
            output.window_transactions[window] += 1;
            output.window_records[window] += batch as u64;
            output.response_ns_log2[(63 - response_ns.max(1).leading_zeros()) as usize] += 1;
        }
    }
    Ok(output)
}

fn verify(db: &Database, workers: &[WorkerResult], seed: u64) -> Result<u64, String> {
    let read = db.begin_read().map_err(|e| e.to_string())?;
    let table = read.open_table(TABLE).map_err(|e| e.to_string())?;
    let mut next = [0_u64; WORKERS];
    let mut total = 0_u64;
    for pair in table.iter().map_err(|e| e.to_string())? {
        let (k, payload) = pair.map_err(|e| e.to_string())?;
        let k = k.value();
        let owner = (k >> 56) as usize;
        if owner >= WORKERS || (k & ((1_u64 << 56) - 1)) != next[owner]
            || payload.value() != value(seed, k) {
            return Err(format!("missing/duplicate/out-of-order key or bad payload near key {k}"));
        }
        next[owner] += 1;
        total += 1;
    }
    for (id, worker) in workers.iter().enumerate() {
        if next[id] != worker.completed_records {
            return Err(format!("worker {id}: expected {} records, read {}", worker.completed_records, next[id]));
        }
    }
    drop(table);
    read.close().map_err(|e| e.to_string())?;
    Ok(total)
}

#[derive(Serialize)]
struct Output {
    variant: String,
    cohort: String,
    durability: String,
    seed: u64,
    smoke_transactions: Option<u64>,
    duration_ms: u64,
    window_ms: u64,
    max_records: u64,
    max_records_per_worker: u64,
    elapsed_ns: u64,
    process_cpu_ns: u64,
    workers: Vec<WorkerResult>,
    verified_live_records: u64,
    reopened_exact: bool,
    reopen_error: Option<String>,
}

pub fn option(name: &str, args: &[String]) -> Result<String, String> {
    args.windows(2).find(|pair| pair[0] == name).map(|pair| pair[1].clone())
        .ok_or_else(|| format!("missing {name}"))
}

fn parse_cpus(value: &str) -> Result<[usize; WORKERS], String> {
    let cpus: Vec<usize> = value.split(',').map(|cpu| cpu.parse::<usize>()
        .map_err(|_| format!("invalid CPU in --cpus: {cpu}"))).collect::<Result<_, _>>()?;
    let cpus: [usize; WORKERS] = cpus.try_into()
        .map_err(|_| format!("--cpus requires exactly {WORKERS} CPUs"))?;
    if cpus.iter().any(|&cpu| cpu >= libc::CPU_SETSIZE as usize)
        || cpus.iter().enumerate().any(|(i, cpu)| cpus[..i].contains(cpu)) {
        return Err("--cpus requires distinct CPU IDs supported by the OS affinity mask".into());
    }
    Ok(cpus)
}

pub fn parse_durability(name: &str) -> Result<Durability, String> {
    match name {
        "immediate" => Ok(Durability::Immediate),
        "none" => Ok(Durability::None),
        _ => Err("durability must be immediate or none".into()),
    }
}

fn trial(args: &[String]) -> Result<(), Box<dyn Error + Send + Sync>> {
    let variant = Variant::parse(&option("--variant", args)?)?;
    let cohort = option("--cohort", args)?;
    let durability_name = option("--durability", args)?;
    let durability = parse_durability(&durability_name)?;
    let seed: u64 = option("--seed", args)?.parse()?;
    let duration_ms: u64 = option("--duration-ms", args)?.parse()?;
    let db_path = PathBuf::from(option("--database", args)?);
    let cpus = parse_cpus(&option("--cpus", args)?)?;
    let smoke_transactions = if args.iter().any(|arg| arg == "--smoke-transactions") {
        Some(option("--smoke-transactions", args)?.parse::<u64>()?)
    } else { None };
    if duration_ms != 2_000 && smoke_transactions.is_none() {
        return Err("timed trials require exactly 2000 ms".into());
    }
    if duration_ms == 0 || smoke_transactions == Some(0) {
        return Err("duration and smoke transactions must be positive".into());
    }
    let batches: [usize; WORKERS] = match cohort.as_str() {
        "all1" => [1; WORKERS],
        "half1_half8" => [1, 1, 1, 1, 8, 8, 8, 8],
        "half1_half64" => [1, 1, 1, 1, 64, 64, 64, 64],
        _ => return Err("unknown cohort".into()),
    };
    let mut affinity: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(&affinity), &mut affinity) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if !(0..libc::CPU_SETSIZE as usize).all(|cpu| {
        let allowed = unsafe { libc::CPU_ISSET(cpu, &affinity) };
        allowed == cpus.contains(&cpu)
    }) {
        return Err(format!("process affinity must be exactly the requested CPUs {cpus:?}").into());
    }
    let db = create_database(&db_path)?;
    let writer = Writer::new(&db, variant)?;
    let barrier = Barrier::new(WORKERS + 1);
    let start = OnceLock::new();
    let duration = Duration::from_millis(duration_ms);
    let (workers, elapsed_ns, process_cpu_ns) = thread::scope(|scope| -> Result<_, Box<dyn Error + Send + Sync>> {
        let handles: Vec<_> = batches.iter().enumerate().map(|(id, &batch)| {
            let (writer, barrier, start) = (&writer, &barrier, &start);
            scope.spawn(move || worker(id, cpus[id], batch, seed, durability, writer, barrier, start,
                                       duration, smoke_transactions))
        }).collect();
        barrier.wait();
        let cpu_begin = cpu_time_ns()?;
        let begin = Instant::now();
        start.set(begin).map_err(|_| "start set twice")?;
        barrier.wait();
        let mut workers = Vec::with_capacity(WORKERS);
        let mut failures = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(Ok(result)) => workers.push(result),
                Ok(Err(error)) => failures.push(error),
                Err(_) => failures.push("worker panic".to_string()),
            }
        }
        let elapsed_ns = begin.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let process_cpu_ns = cpu_time_ns()? - cpu_begin;
        if !failures.is_empty() { return Err(failures.join("; ").into()); }
        Ok((workers, elapsed_ns, process_cpu_ns))
    })?;
    // Reads stay outside the write gate: verification runs while it is still held.
    let live = verify(&db, &workers, seed)?;
    drop(writer);
    drop(db);
    // Close/reopen checks normal process-close persistence, NOT crash/power-loss durability.
    let (reopened_exact, reopen_error) = match Database::open(&db_path) {
        Ok(reopened) => match verify(&reopened, &workers, seed) {
            Ok(n) if n == live => (true, None),
            Ok(n) => (false, Some(format!("reopened {n}, live {live}"))),
            Err(error) => (false, Some(error)),
        },
        Err(error) => (false, Some(error.to_string())),
    };
    let output = Output { variant: variant.name().into(), cohort, durability: durability_name, seed,
        smoke_transactions, duration_ms, window_ms: WINDOW_MS, max_records: MAX_RECORDS,
        max_records_per_worker: MAX_RECORDS_PER_WORKER, elapsed_ns, process_cpu_ns,
        workers, verified_live_records: live, reopened_exact, reopen_error };
    println!("{}", serde_json::to_string(&output)?);
    if output.durability == "immediate" && !output.reopened_exact {
        return Err("Immediate close/reopen verification failed".into());
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.iter().any(|arg| arg == "--self-test") {
        selftest::run(&args)
    } else {
        trial(&args)
    };
    if let Err(error) = result {
        eprintln!("redb write trial failed: {error}");
        std::process::exit(1);
    }
}
