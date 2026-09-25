//! Eight saturated, pinned requesters, each issuing complete redb write transactions.
//! Only the write request is delegated; Database::begin_read remains available to
//! any caller (read transactions are not protected by the scheduling lock).
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    error::Error,
    fs,
    path::PathBuf,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use libdlock::{
    dlock2::{
        fc::FC, fc_pq::{FCPQ, UsageNode}, mcs::RawMcsLock,
        spinlock::DLock2Wrapper, DLock2,
    },
    spin_lock::RawSpinLock,
};
use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use serde::Serialize;

const WORKERS: usize = 8;
const DEFAULT_CPUS: [usize; WORKERS] = [8, 9, 10, 11, 12, 13, 14, 15];
const TABLE: TableDefinition<u64, u64> = TableDefinition::new("writer_records");
const MAX_RECORDS: u64 = 64_000_000;
const MAX_RECORDS_PER_WORKER: u64 = MAX_RECORDS / WORKERS as u64;
const WINDOW_MS: u64 = 250;
const WINDOWS: usize = 8;

type Delegate = fn(&mut Arc<Database>, Request) -> Request;
type Priority = BinaryHeap<Reverse<UsageNode<'static, Request>>>;

enum Scheduler {
    Native(Arc<Database>),
    Mutex(std::sync::Mutex<Arc<Database>>),
    Mcs(DLock2Wrapper<Arc<Database>, Request, Delegate, RawMcsLock>),
    Fc(FC<Arc<Database>, Request, Delegate>),
    FcPq(FCPQ<Arc<Database>, Request, Priority, Delegate, RawSpinLock>),
}

impl Scheduler {
    fn submit(&self, request: Request) -> Request {
        match self {
            Self::Native(db) => execute(db, request),
            Self::Mutex(db) => execute(&db.lock().expect("mutex poisoned"), request),
            Self::Mcs(lock) => lock.lock(request),
            Self::Fc(lock) => lock.lock(request),
            Self::FcPq(lock) => lock.lock(request),
        }
    }
}

#[derive(Debug)]
struct Request {
    worker: usize,
    first: u64,
    records: u64,
    seed: u64,
    durability: Durability,
    outcome: Option<Result<(), String>>,
    #[cfg(feature = "redb_profile")]
    service_wall_ns: u64,
}

fn value(seed: u64, key: u64) -> u64 {
    let mut v = seed ^ key.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    v ^= v >> 30;
    v = v.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    v ^= v >> 27;
    v = v.wrapping_mul(0x94d0_49bb_1331_11eb);
    v ^ (v >> 31)
}

fn transaction(db: &Database, req: &mut Request) -> Result<(), Box<dyn Error + Send + Sync>> {
    // The transaction is constructed, populated and committed on this executor.
    // In particular, no thread sends a redb transaction/guard to another thread.
    let mut txn = db.begin_write()?;
    // begin_write waits on the native writer lock; that wait is requester latency,
    // not protected service. The measured wall interval includes commit I/O.
    #[cfg(feature = "redb_profile")]
    let service_start = Instant::now();
    txn.set_durability(req.durability)?;
    {
        let mut table = txn.open_table(TABLE)?;
        for offset in 0..req.records {
            let key = ((req.worker as u64) << 56) | (req.first + offset);
            if table.insert(key, value(req.seed, key))?.is_some() {
                return Err(format!("duplicate key {key}").into());
            }
        }
    }
    txn.commit()?;
    #[cfg(feature = "redb_profile")]
    {
        req.service_wall_ns = service_start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    }
    Ok(())
}

fn execute(db: &Database, mut req: Request) -> Request {
    let outcome = transaction(db, &mut req);
    req.outcome = Some(outcome.map_err(|error| error.to_string()));
    req
}

fn delegated(db: &mut Arc<Database>, req: Request) -> Request {
    execute(db, req)
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
    #[cfg(feature = "redb_profile")]
    service_wall_ns: u64,
    #[cfg(feature = "redb_profile")]
    #[serde(serialize_with = "serialize_histogram")]
    service_ns_log2: [u64; 64],
}

fn serialize_histogram<S: serde::Serializer>(values: &[u64; 64], serializer: S) -> Result<S::Ok, S::Error> {
    values.as_slice().serialize(serializer)
}

fn bucket(histogram: &mut [u64; 64], ns: u64) {
    histogram[(63 - ns.max(1).leading_zeros()) as usize] += 1;
}

fn cpu_time_ns() -> Result<u64, Box<dyn Error + Send + Sync>> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((time.tv_sec as u64) * 1_000_000_000 + time.tv_nsec as u64)
}

fn worker(
    id: usize, cpu: usize, batch: u64, seed: u64, durability: Durability,
    scheduler: Arc<Scheduler>, barrier: Arc<Barrier>,
    start: Arc<std::sync::OnceLock<Instant>>,
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
        cpu, records_per_transaction: batch,
        completed_transactions: 0, completed_records: 0,
        window_transactions: [0; WINDOWS], window_records: [0; WINDOWS],
        response_ns_log2: [0; 64],
        #[cfg(feature = "redb_profile")]
        service_wall_ns: 0,
        #[cfg(feature = "redb_profile")]
        service_ns_log2: [0; 64],
    };
    loop {
        if let Some(limit) = smoke_transactions {
            if output.completed_transactions >= limit { break; }
        } else if begin.elapsed() >= duration { break; }
        if output.completed_records + batch > MAX_RECORDS_PER_WORKER {
            return Err(format!("worker {id} reached {MAX_RECORDS_PER_WORKER}-record cap; trial invalid"));
        }
        let req = Request {
            worker: id, first: output.completed_records, records: batch, seed, durability,
            outcome: None,
            #[cfg(feature = "redb_profile")]
            service_wall_ns: 0,
        };
        let submitted = Instant::now();
        let mut reply = scheduler.submit(req);
        let response_ns = submitted.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let completed = begin.elapsed();
        reply.outcome.take().ok_or("executor did not return a result")?
            .map_err(|error| format!("worker {id}, transaction {}: {error}", output.completed_transactions))?;
        output.completed_transactions += 1;
        output.completed_records += batch;
        // Service allocation credits only requests completed within the fixed
        // window; a draining request remains in the final database oracle.
        if completed < duration || smoke_transactions.is_some() {
            let window = ((completed.as_millis() / WINDOW_MS as u128) as usize).min(WINDOWS - 1);
            output.window_transactions[window] += 1;
            output.window_records[window] += batch;
            bucket(&mut output.response_ns_log2, response_ns);
            #[cfg(feature = "redb_profile")]
            {
                output.service_wall_ns += reply.service_wall_ns;
                bucket(&mut output.service_ns_log2, reply.service_wall_ns);
            }
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
        let (key, payload) = pair.map_err(|e| e.to_string())?;
        let key = key.value();
        let owner = (key >> 56) as usize;
        if owner >= WORKERS || (key & ((1_u64 << 56) - 1)) != next[owner]
            || payload.value() != value(seed, key) {
            return Err(format!("missing/duplicate/out-of-order key or bad payload near key {key}"));
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
    backend: String,
    cohort: String,
    durability: String,
    seed: u64,
    profile: bool,
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

fn option(name: &str, args: &[String]) -> Result<String, String> {
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

fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let backend = option("--backend", &args)?;
    let cohort = option("--cohort", &args)?;
    let durability_name = option("--durability", &args)?;
    let seed: u64 = option("--seed", &args)?.parse()?;
    let duration_ms: u64 = option("--duration-ms", &args)?.parse()?;
    let db_path = PathBuf::from(option("--database", &args)?);
    let cpus = if args.iter().any(|arg| arg == "--cpus") {
        parse_cpus(&option("--cpus", &args)?)?
    } else {
        DEFAULT_CPUS
    };
    let smoke_transactions = if args.iter().any(|arg| arg == "--smoke-transactions") {
        Some(option("--smoke-transactions", &args)?.parse::<u64>()?)
    } else { None };
    if duration_ms != 2_000 && smoke_transactions.is_none() {
        return Err("formal trials require exactly 2000 ms".into());
    }
    if duration_ms == 0 || smoke_transactions == Some(0) {
        return Err("duration and smoke transactions must be positive".into());
    }
    let durability = match durability_name.as_str() {
        "immediate" => Durability::Immediate,
        "none" => Durability::None,
        _ => return Err("durability must be immediate or none".into()),
    };
    let batches: [u64; WORKERS] = match cohort.as_str() {
        "all1" => [1; WORKERS],
        "half1_half8" => [1, 1, 1, 1, 8, 8, 8, 8],
        "half1_half64" => [1, 1, 1, 1, 64, 64, 64, 64],
        _ => return Err("unknown cohort".into()),
    };
    if db_path.exists() { return Err(format!("database already exists: {}", db_path.display()).into()); }
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
    fs::create_dir_all(db_path.parent().ok_or("database needs a parent directory")?)?;
    let db = Arc::new(Database::create(&db_path)?);
    {
        let mut txn = db.begin_write()?;
        txn.set_durability(Durability::Immediate)?;
        { let _table = txn.open_table(TABLE)?; }
        txn.commit()?; // Table creation is outside the measured window.
    }
    let scheduler = Arc::new(match backend.as_str() {
        "native" => Scheduler::Native(Arc::clone(&db)),
        "mutex" => Scheduler::Mutex(std::sync::Mutex::new(Arc::clone(&db))),
        "mcs" => Scheduler::Mcs(DLock2Wrapper::new(Arc::clone(&db), delegated as Delegate)),
        "fc" => Scheduler::Fc(FC::new(Arc::clone(&db), delegated as Delegate)),
        "fc_pq" => Scheduler::FcPq(FCPQ::new(Arc::clone(&db), delegated as Delegate)),
        _ => return Err(format!("unknown backend: {backend}").into()),
    });
    let barrier = Arc::new(Barrier::new(WORKERS + 1));
    let start = Arc::new(std::sync::OnceLock::new());
    let mut handles = Vec::with_capacity(WORKERS);
    for (id, &batch) in batches.iter().enumerate() {
        let (s, b, t) = (Arc::clone(&scheduler), Arc::clone(&barrier), Arc::clone(&start));
        handles.push(thread::spawn(move || worker(id, cpus[id], batch, seed, durability, s, b, t,
            Duration::from_millis(duration_ms), smoke_transactions)));
    }
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
    let live = verify(&db, &workers, seed)?;
    drop(scheduler);
    drop(db);
    // Close/reopen checks normal process-close persistence, NOT crash/power-loss durability.
    // Durability::None explicitly does not promise persistence; record rather than suppress loss.
    let (reopened_exact, reopen_error) = match Database::open(&db_path) {
        Ok(reopened) => match verify(&reopened, &workers, seed) {
            Ok(n) if n == live => (true, None),
            Ok(n) => (false, Some(format!("reopened {n}, live {live}"))),
            Err(error) => (false, Some(error)),
        },
        Err(error) => (false, Some(error.to_string())),
    };
    let output = Output { backend, cohort, durability: durability_name, seed,
        profile: cfg!(feature = "redb_profile"), smoke_transactions, duration_ms,
        window_ms: WINDOW_MS, max_records: MAX_RECORDS,
        max_records_per_worker: MAX_RECORDS_PER_WORKER, elapsed_ns, process_cpu_ns,
        workers, verified_live_records: live, reopened_exact, reopen_error };
    println!("{}", serde_json::to_string(&output)?);
    if output.durability == "immediate" && !output.reopened_exact {
        return Err("Immediate close/reopen verification failed".into());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("redb transaction trial failed: {error}");
        std::process::exit(1);
    }
}
