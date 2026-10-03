//! One to sixty-four saturated requesters, one pinned per `--cpus` entry, each
//! submitting whole redb write transactions as closures through one of seven
//! variants (see writer.rs). Read transactions (`Database::begin_read`) never
//! pass through any write lock or bridge. Each worker is charged the service
//! time of its own bodies, wherever they ran. Optional reader threads
//! (`--reader-cpus`) run read transactions on their own CPUs during the same
//! window (see `reader`).
#[cfg(all(feature = "upstream", feature = "patched"))]
compile_error!("build exactly one of the `upstream` and `patched` features");
#[cfg(not(any(feature = "upstream", feature = "patched")))]
compile_error!("build exactly one of the `upstream` and `patched` features");

#[cfg(feature = "upstream")]
extern crate redb_upstream as redb;

mod selftest;
mod writer;

use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Barrier, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use redb::{Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata};
use serde::Serialize;

use writer::{
    transfer_body, Variant, WriteError, Writer, ACCOUNTS, MAX_RECORDS_PER_REQUEST, TABLE,
};

const MAX_WORKERS: usize = 64;
/// Default read transaction: `READER_GETS` point gets and one scan of `READER_SCAN` entries.
const READER_GETS: usize = 4;
const READER_SCAN: usize = 16;
/// Every `READER_STALE_EVERY`-th read transaction samples snapshot staleness.
const READER_STALE_EVERY: u64 = 16;
/// Every `READER_DISCOVER_EVERY`-th read transaction probes a random writer for data.
const READER_DISCOVER_EVERY: u64 = 64;
const MAX_RECORDS_PER_WORKER: u64 = 8_000_000;
const MAX_RECORDS: u64 = MAX_WORKERS as u64 * MAX_RECORDS_PER_WORKER;
const WINDOW_MS: u64 = 250;
const WINDOWS: usize = 8;

/// Transfer cohort: fixed account set, initial balance and amount range.
pub const TRANSFER_ACCOUNTS: u64 = 1024;
pub const INITIAL_BALANCE: u64 = 1_000;
pub const MAX_TRANSFER: u64 = 500;

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
pub fn records(
    buffer: &mut [(u64, u64); MAX_RECORDS_PER_REQUEST],
    worker: usize,
    first: u64,
    count: usize,
    seed: u64,
) -> &[(u64, u64)] {
    for (offset, slot) in buffer[..count].iter_mut().enumerate() {
        let k = key(worker, first + offset as u64);
        *slot = (k, value(seed, k));
    }
    &buffer[..count]
}

/// Per-requester transfer stream (from, to, amount), generated outside any lock.
pub struct Transfers {
    state: u64,
    accounts: u64,
}

impl Transfers {
    pub fn new(seed: u64, worker: usize, accounts: u64) -> Self {
        Self {
            state: value(seed, worker as u64 + 1),
            accounts,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        value(0, self.state)
    }

    pub fn next_transfer(&mut self) -> (u64, u64, u64) {
        let from = self.next_u64() % self.accounts;
        let to = (from + 1 + self.next_u64() % (self.accounts - 1)) % self.accounts;
        (from, to, 1 + self.next_u64() % MAX_TRANSFER)
    }
}

pub fn create_database(path: &PathBuf) -> Result<Database, Box<dyn Error + Send + Sync>> {
    if path.exists() {
        return Err(format!("database already exists: {}", path.display()).into());
    }
    fs::create_dir_all(path.parent().ok_or("database needs a parent directory")?)?;
    let db = Database::create(path)?;
    let mut txn = db.begin_write()?;
    txn.set_durability(Durability::Immediate)?;
    {
        let _table = txn.open_table(TABLE)?;
    }
    txn.commit()?; // Table creation precedes any gate and the measured window.
    Ok(db)
}

/// Create the transfer accounts through the public API, before any gate.
pub fn seed_accounts(db: &Database, accounts: u64) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut txn = db.begin_write()?;
    txn.set_durability(Durability::Immediate)?;
    {
        let mut table = txn.open_table(ACCOUNTS)?;
        for account in 0..accounts {
            table.insert(account, INITIAL_BALANCE)?;
        }
    }
    txn.commit()?;
    Ok(())
}

/// All balances in one read snapshot; the account set and total must be conserved.
pub fn conserved_balances(db: &Database, accounts: u64) -> Result<Vec<u64>, String> {
    let read = db.begin_read().map_err(|e| e.to_string())?;
    let table = read.open_table(ACCOUNTS).map_err(|e| e.to_string())?;
    let mut balances = Vec::with_capacity(accounts as usize);
    for (index, pair) in table.iter().map_err(|e| e.to_string())?.enumerate() {
        let (account, balance) = pair.map_err(|e| e.to_string())?;
        if account.value() != index as u64 {
            return Err(format!(
                "account set changed near account {}",
                account.value()
            ));
        }
        balances.push(balance.value());
    }
    let total: u64 = balances.iter().sum();
    if balances.len() as u64 != accounts || total != accounts * INITIAL_BALANCE {
        return Err(format!(
            "not conserved: {} accounts, total {total}, expected {} and {}",
            balances.len(),
            accounts,
            accounts * INITIAL_BALANCE
        ));
    }
    if table.len().map_err(|e| e.to_string())? != accounts {
        return Err("account table length changed".into());
    }
    Ok(balances)
}

/// Features compiled into this binary (`--build-info`; echoed in every trial).
#[derive(Serialize)]
struct BuildInfo {
    upstream: bool,
    patched: bool,
    test_hooks: bool,
    service_time: bool,
    fcpq_fast_path: bool,
    fcpq_fast_path_stat: bool,
}

const BUILD_INFO: BuildInfo = BuildInfo {
    upstream: cfg!(feature = "upstream"),
    patched: cfg!(feature = "patched"),
    test_hooks: cfg!(feature = "test_hooks"),
    service_time: cfg!(feature = "service_time"),
    fcpq_fast_path: cfg!(feature = "fcpq_fast_path"),
    fcpq_fast_path_stat: cfg!(feature = "fcpq_fast_path_stat"),
};

/// Profile cohort only (run.py --perf): `perf stat --delay=-1 --control
/// fifo:CTL,ACK` starts disabled, and REDB_PERF_CONTROL=CTL,ACK makes this
/// process enable counting while the clients run and disable it once they
/// are joined. Setup, verification and close/reopen are therefore not
/// counted. Timed cells never set the variable, so there perf is absent.
struct PerfControl {
    ctl: fs::File,
    ack: std::io::BufReader<fs::File>,
}

impl PerfControl {
    fn from_env() -> Result<Option<Self>, Box<dyn Error + Send + Sync>> {
        let Ok(spec) = std::env::var("REDB_PERF_CONTROL") else {
            return Ok(None);
        };
        let (ctl, ack) = spec
            .split_once(',')
            .ok_or("REDB_PERF_CONTROL must be CTL,ACK")?;
        Ok(Some(Self {
            ctl: fs::OpenOptions::new().write(true).open(ctl)?,
            ack: std::io::BufReader::new(fs::File::open(ack)?),
        }))
    }

    fn send(&mut self, command: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        use std::io::{BufRead, Write};
        self.ctl.write_all(format!("{command}\n").as_bytes())?;
        self.ctl.flush()?;
        let mut reply = String::new();
        self.ack.read_line(&mut reply)?;
        // perf writes "ack\n" plus its terminating NUL, which lands in front of the next reply.
        if !reply.trim_start_matches('\0').starts_with("ack") {
            return Err(format!("perf control {command}: unexpected reply {reply:?}").into());
        }
        Ok(())
    }
}

pub fn tsc() -> u64 {
    let mut aux = 0_u32;
    // SAFETY: rdtscp only writes `aux`; this harness targets x86_64.
    unsafe { std::arch::x86_64::__rdtscp(&mut aux) }
}

#[derive(Clone)]
enum Workload {
    /// Fixed inserts; records per request, cycled in order (one entry unless
    /// the cohort's 1/K mix has to live in a single client).
    Insert(Vec<usize>),
    /// One transfer (2 reads + 2 updates) per transaction.
    Transfer,
}

impl Workload {
    /// Records per request as reported per worker (`request_sizes`).
    fn request_sizes(&self) -> Vec<u64> {
        match self {
            Self::Insert(sizes) => sizes.iter().map(|&n| n as u64).collect(),
            Self::Transfer => vec![2],
        }
    }

    /// Records written by the worker's `sequence`-th committed transaction.
    fn batch(&self, sequence: u64) -> usize {
        match self {
            Self::Insert(sizes) => sizes[(sequence % sizes.len() as u64) as usize],
            Self::Transfer => 2,
        }
    }
}

#[derive(Serialize)]
struct WorkerResult {
    cpu: usize,
    /// Records per request, cycled in order (one entry unless the cohort's mix
    /// has to live in a single client); `[2]` for transfer (two updates).
    request_sizes: Vec<u64>,
    /// Committed transactions.
    completed_transactions: u64,
    completed_records: u64,
    /// Transfer cohort: transactions aborted for insufficient balance (not in windows).
    aborted_transactions: u64,
    window_transactions: [u64; WINDOWS],
    window_records: [u64; WINDOWS],
    /// Service TSC ticks of this worker's committed bodies credited to the
    /// window (same requests as `window_transactions`), wherever they ran.
    service_tsc_ticks: u64,
    /// FC-PQ fast-path hits over all of this worker's requests (fc_pq with
    /// `fcpq_fast_path_stat` only).
    fast_path_hits: Option<u64>,
    /// Committed transactions only.
    #[serde(serialize_with = "serialize_histogram")]
    response_ns_log2: [u64; 64],
}

fn serialize_histogram<S: serde::Serializer>(
    values: &[u64; 64],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    values.as_slice().serialize(serializer)
}

fn cpu_time_ns() -> Result<u64, Box<dyn Error + Send + Sync>> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((time.tv_sec as u64) * 1_000_000_000 + time.tv_nsec as u64)
}

#[allow(clippy::too_many_arguments)]
fn worker(
    id: usize,
    cpu: usize,
    workload: &Workload,
    seed: u64,
    durability: Durability,
    writer: &Writer<'_>,
    barrier: &Barrier,
    start: &OnceLock<Instant>,
    duration: Duration,
    smoke_transactions: Option<u64>,
) -> Result<WorkerResult, String> {
    let pinned = core_affinity::set_for_current(core_affinity::CoreId { id: cpu });
    barrier.wait();
    // The first rendezvous reports readiness; the second publishes the clock.
    barrier.wait();
    if !pinned {
        return Err(format!("cannot pin worker {id} to CPU {cpu}"));
    }
    let begin = *start
        .get()
        .expect("main initialized clock between barriers");
    let mut output = WorkerResult {
        cpu,
        request_sizes: workload.request_sizes(),
        completed_transactions: 0,
        completed_records: 0,
        aborted_transactions: 0,
        window_transactions: [0; WINDOWS],
        window_records: [0; WINDOWS],
        service_tsc_ticks: 0,
        fast_path_hits: None,
        response_ns_log2: [0; 64],
    };
    let mut buffer = [(0, 0); MAX_RECORDS_PER_REQUEST];
    let mut transfers = Transfers::new(seed, id, TRANSFER_ACCOUNTS);
    loop {
        if let Some(limit) = smoke_transactions {
            if output.completed_transactions >= limit {
                break;
            }
        } else if begin.elapsed() >= duration {
            break;
        }
        // Requests cycle through the client's sizes (one size except a lone mixed client).
        let count = workload.batch(output.completed_transactions);
        let batch = count as u64;
        if output.completed_records + batch > MAX_RECORDS_PER_WORKER {
            return Err(format!(
                "worker {id} reached {MAX_RECORDS_PER_WORKER}-record cap; trial invalid"
            ));
        }
        let (committed, response_ns, completed) = match workload {
            Workload::Insert(_) => {
                let request = records(&mut buffer, id, output.completed_records, count, seed);
                let submitted = Instant::now();
                let outcome = writer.insert(request, durability);
                let response_ns = submitted.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                let completed = begin.elapsed();
                let written = outcome.map_err(|error| {
                    format!(
                        "worker {id}, transaction {}: {error}",
                        output.completed_transactions
                    )
                })?;
                (Some(written.service_tsc), response_ns, completed)
            }
            Workload::Transfer => {
                let (from, to, amount) = transfers.next_transfer();
                let submitted = Instant::now();
                let outcome = writer.write(transfer_body(from, to, amount, durability));
                let response_ns = submitted.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                let completed = begin.elapsed();
                match outcome {
                    Ok(written) => (Some(written.service_tsc), response_ns, completed),
                    // The aborted body held the lock too, but only committed
                    // bodies return their service time; see README "Known limits".
                    Err(WriteError::Insufficient { .. }) => (None, response_ns, completed),
                    Err(error) => return Err(format!("worker {id}, transfer: {error}")),
                }
            }
        };
        let Some(service_tsc) = committed else {
            output.aborted_transactions += 1;
            continue;
        };
        output.completed_transactions += 1;
        output.completed_records += batch;
        // Only requests completed within the fixed window are credited; a draining
        // request remains in the final database oracle.
        if completed < duration || smoke_transactions.is_some() {
            let window = ((completed.as_millis() / WINDOW_MS as u128) as usize).min(WINDOWS - 1);
            output.window_transactions[window] += 1;
            output.window_records[window] += batch;
            output.service_tsc_ticks += service_tsc;
            output.response_ns_log2[(63 - response_ns.max(1).leading_zeros()) as usize] += 1;
        }
    }
    output.fast_path_hits = writer.fast_path_hits();
    Ok(output)
}

/// Fine latency histogram: values below 16 ns index directly; above, 8 linear
/// sub-buckets per power of two (bucket width <= 12.5 % of its lower bound).
const FINE_BUCKETS: usize = 64 * 8;

fn fine_index(ns: u64) -> usize {
    let v = ns.max(1);
    if v < 16 {
        return v as usize;
    }
    let shift = 63 - v.leading_zeros() - 3;
    (((shift + 1) << 3) + ((v >> shift) as u32 - 8)) as usize
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[derive(Clone, Copy, Serialize)]
struct ReaderConfig {
    gets: usize,
    scan: usize,
}

#[derive(Serialize)]
struct ReaderResult {
    cpu: usize,
    /// Completed read transactions (begin_read .. last read done).
    read_txns: u64,
    /// Point gets, scans (one per read transaction with a non-empty writer range) and scanned entries.
    gets: u64,
    scans: u64,
    scan_entries: u64,
    /// Discovery probes that hit a writer with no record in the snapshot yet.
    empty_writer_ranges: u64,
    /// Records in the reader's snapshots (`table.len()`): smallest and largest seen.
    snapshot_len_min: u64,
    snapshot_len_max: u64,
    /// Staleness sampled on every READER_STALE_EVERY-th transaction: records committed
    /// between the snapshot and a second begin_read after the transaction's reads.
    stale_samples: u64,
    stale_sum: u64,
    stale_max: u64,
    stale_nonzero: u64,
    /// Whole read transaction and `begin_read` alone, fine log histograms (ns).
    txn_ns_hist: Vec<u64>,
    begin_ns_hist: Vec<u64>,
}

/// Largest sequence number of `owner`'s keys in the snapshot (`None`: none yet).
fn last_seq(table: &redb::ReadOnlyTable<u64, u64>, owner: u64) -> Result<Option<u64>, String> {
    const SEQ_MASK: u64 = (1 << 56) - 1;
    let mut range = table
        .range(owner..=(owner | SEQ_MASK))
        .map_err(|e| e.to_string())?;
    match range.next_back() {
        None => Ok(None),
        Some(pair) => {
            let (k, _) = pair.map_err(|e| e.to_string())?;
            Ok(Some(k.value() & SEQ_MASK))
        }
    }
}

/// One reader: loops read transactions until the window ends (timed) or the
/// writers are done (smoke). Each transaction reads one snapshot: the chosen
/// writer's last key in it (a writer already known to have data), `gets` point
/// gets of random keys of that writer below it, and a scan of `scan` consecutive keys. A writer's keys are
/// contiguous from 0, so every get must hit with the exact payload and the scan
/// must be consecutive with exact payloads; anything else fails the cell. No
/// state is shared with the writers (no counter that would add coherence traffic
/// to their cache lines), and readers never enter a write lock.
#[allow(clippy::too_many_arguments)]
fn reader(
    id: usize,
    cpu: usize,
    db: &Database,
    config: ReaderConfig,
    writers: usize,
    seed: u64,
    barrier: &Barrier,
    start: &OnceLock<Instant>,
    duration: Duration,
    done: &AtomicBool,
    smoke: bool,
) -> Result<ReaderResult, String> {
    let pinned = core_affinity::set_for_current(core_affinity::CoreId { id: cpu });
    barrier.wait();
    barrier.wait();
    if !pinned {
        return Err(format!("cannot pin reader {id} to CPU {cpu}"));
    }
    let begin = *start
        .get()
        .expect("main initialized clock between barriers");
    let mut rng = seed ^ 0x5eed_0000_0000_0000 ^ ((id as u64 + 1) << 32);
    let mut out = ReaderResult {
        cpu,
        read_txns: 0,
        gets: 0,
        scans: 0,
        scan_entries: 0,
        empty_writer_ranges: 0,
        snapshot_len_min: u64::MAX,
        snapshot_len_max: 0,
        stale_samples: 0,
        stale_sum: 0,
        stale_max: 0,
        stale_nonzero: 0,
        txn_ns_hist: vec![0; FINE_BUCKETS],
        begin_ns_hist: vec![0; FINE_BUCKETS],
    };
    const SEQ_MASK: u64 = (1 << 56) - 1;
    let fail = |what: String| format!("reader {id}: {what}");
    let mut known: Vec<u64> = Vec::new();
    loop {
        if smoke {
            if done.load(Ordering::Acquire) {
                break;
            }
        } else if begin.elapsed() >= duration {
            break;
        }
        let started = Instant::now();
        let read = db.begin_read().map_err(|e| fail(e.to_string()))?;
        let begin_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let table = read.open_table(TABLE).map_err(|e| fail(e.to_string()))?;
        let len = table.len().map_err(|e| fail(e.to_string()))?;
        // Reader work must not depend on how evenly the writers' lock shares the table:
        // keys are never deleted, so a writer with data in an earlier snapshot stays
        // non-empty. Transactions read a random known writer; every
        // READER_DISCOVER_EVERY-th one (and while none is known) first probes a random
        // writer to learn about more.
        let mut chosen = None;
        if known.is_empty() || out.read_txns % READER_DISCOVER_EVERY == 0 {
            let w = splitmix(&mut rng) % writers as u64;
            match last_seq(&table, w << 56).map_err(&fail)? {
                Some(max_seq) => {
                    if !known.contains(&w) {
                        known.push(w);
                    }
                    chosen = Some((w << 56, max_seq));
                }
                None => out.empty_writer_ranges += 1,
            }
        }
        if chosen.is_none() && !known.is_empty() {
            let w = known[(splitmix(&mut rng) % known.len() as u64) as usize];
            match last_seq(&table, w << 56).map_err(&fail)? {
                Some(max_seq) => chosen = Some((w << 56, max_seq)),
                None => return Err(fail(format!("writer {w}'s key range became empty"))),
            }
        }
        match chosen {
            None => {}
            Some((owner, max_seq)) => {
                for _ in 0..config.gets {
                    let key = owner | (splitmix(&mut rng) % (max_seq + 1));
                    match table.get(key).map_err(|e| fail(e.to_string()))? {
                        Some(v) if v.value() == value(seed, key) => {}
                        Some(v) => {
                            return Err(fail(format!("key {key:#x} has payload {}", v.value())))
                        }
                        None => {
                            return Err(fail(format!(
                                "key {key:#x} <= last key of its writer is missing"
                            )))
                        }
                    }
                    out.gets += 1;
                }
                if config.scan > 0 {
                    let first = splitmix(&mut rng) % (max_seq + 1);
                    let mut expected = first;
                    let range = table
                        .range((owner | first)..=(owner | SEQ_MASK))
                        .map_err(|e| fail(e.to_string()))?;
                    for pair in range.take(config.scan) {
                        let (k, v) = pair.map_err(|e| fail(e.to_string()))?;
                        let key = k.value();
                        if key != (owner | expected) || v.value() != value(seed, key) {
                            return Err(fail(format!(
                                "scan from {:#x}: got key {key:#x} or bad payload, expected {:#x}",
                                owner | first,
                                owner | expected
                            )));
                        }
                        expected += 1;
                        out.scan_entries += 1;
                    }
                    out.scans += 1;
                }
            }
        }
        let txn_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        out.read_txns += 1;
        out.snapshot_len_min = out.snapshot_len_min.min(len);
        out.snapshot_len_max = out.snapshot_len_max.max(len);
        out.txn_ns_hist[fine_index(txn_ns)] += 1;
        out.begin_ns_hist[fine_index(begin_ns)] += 1;
        drop(table);
        if out.read_txns % READER_STALE_EVERY == 0 {
            let fresh = db.begin_read().map_err(|e| fail(e.to_string()))?;
            let fresh_table = fresh.open_table(TABLE).map_err(|e| fail(e.to_string()))?;
            let fresh_len = fresh_table.len().map_err(|e| fail(e.to_string()))?;
            let stale = fresh_len.saturating_sub(len);
            out.stale_samples += 1;
            out.stale_sum += stale;
            out.stale_max = out.stale_max.max(stale);
            out.stale_nonzero += u64::from(stale > 0);
        }
    }
    if out.read_txns == 0 {
        out.snapshot_len_min = 0;
    }
    Ok(out)
}

fn verify(db: &Database, workers: &[WorkerResult], seed: u64) -> Result<u64, String> {
    let read = db.begin_read().map_err(|e| e.to_string())?;
    let table = read.open_table(TABLE).map_err(|e| e.to_string())?;
    let mut next = vec![0_u64; workers.len()];
    let mut total = 0_u64;
    for pair in table.iter().map_err(|e| e.to_string())? {
        let (k, payload) = pair.map_err(|e| e.to_string())?;
        let k = k.value();
        let owner = (k >> 56) as usize;
        if owner >= workers.len()
            || (k & ((1_u64 << 56) - 1)) != next[owner]
            || payload.value() != value(seed, k)
        {
            return Err(format!(
                "missing/duplicate/out-of-order key or bad payload near key {k}"
            ));
        }
        next[owner] += 1;
        total += 1;
    }
    for (id, worker) in workers.iter().enumerate() {
        if next[id] != worker.completed_records {
            return Err(format!(
                "worker {id}: expected {} records, read {}",
                worker.completed_records, next[id]
            ));
        }
    }
    drop(table);
    read.close().map_err(|e| e.to_string())?;
    Ok(total)
}

#[derive(Serialize)]
struct TransferCheck {
    accounts: u64,
    initial_balance: u64,
    max_transfer: u64,
    total: u64,
    conserved_live: bool,
    reopened_identical: bool,
    committed: u64,
    aborted: u64,
}

#[derive(Serialize)]
struct Output {
    variant: String,
    cohort: String,
    clients: usize,
    cpus: Vec<usize>,
    durability: String,
    seed: u64,
    smoke_transactions: Option<u64>,
    duration_ms: u64,
    window_ms: u64,
    max_records: u64,
    max_records_per_worker: u64,
    elapsed_ns: u64,
    /// Profile cohort: perf counted exactly the client phase (enable after
    /// all clients were ready, disable after all were joined).
    perf_counted: bool,
    /// TSC rate over the measured run, to convert service ticks to time.
    tsc_ticks_per_ns: f64,
    process_cpu_ns: u64,
    build: BuildInfo,
    workers: Vec<WorkerResult>,
    verified_live_records: u64,
    reopened_exact: bool,
    reopen_error: Option<String>,
    transfer: Option<TransferCheck>,
    /// Reader threads (empty without `--reader-cpus`); read transactions never touch a write lock.
    reader_cpus: Vec<usize>,
    reader_config: ReaderConfig,
    readers: Vec<ReaderResult>,
}

pub fn option(name: &str, args: &[String]) -> Result<String, String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .ok_or_else(|| format!("missing {name}"))
}

/// Distinct CPU IDs supported by the OS affinity mask, at most `max` of them.
fn parse_cpu_list(value: &str, flag: &str, max: usize) -> Result<Vec<usize>, String> {
    let cpus: Vec<usize> = value
        .split(',')
        .map(|cpu| {
            cpu.parse::<usize>()
                .map_err(|_| format!("invalid CPU in {flag}: {cpu}"))
        })
        .collect::<Result<_, _>>()?;
    if cpus.is_empty() || cpus.len() > max {
        return Err(format!("{flag} requires 1..={max} CPUs"));
    }
    if cpus.iter().any(|&cpu| cpu >= libc::CPU_SETSIZE as usize)
        || cpus
            .iter()
            .enumerate()
            .any(|(i, cpu)| cpus[..i].contains(cpu))
    {
        return Err(format!(
            "{flag} requires distinct CPU IDs supported by the OS affinity mask"
        ));
    }
    Ok(cpus)
}

/// One client per CPU, 1..=MAX_WORKERS distinct CPUs.
fn parse_cpus(value: &str) -> Result<Vec<usize>, String> {
    parse_cpu_list(value, "--cpus", MAX_WORKERS)
}

/// Workload of each client. `all1`: every client writes 1 record. `half1_halfK`
/// (K = 8 or 64): half the requests have 1 record and half K. With an even
/// client count, clients 0..n/2 write 1 record and clients n/2..n write K
/// (2 clients = one of each); a single client alternates 1, K, 1, K, ... (same
/// request mix, no contention). Other odd counts are rejected. `transfer`:
/// every client runs the transfer closure (any client count).
fn cohort_workloads(cohort: &str, clients: usize) -> Result<Vec<Workload>, String> {
    let large = match cohort {
        "all1" => return Ok(vec![Workload::Insert(vec![1]); clients]),
        "transfer" => return Ok(vec![Workload::Transfer; clients]),
        "half1_half8" => 8,
        "half1_half64" => 64,
        _ => return Err(format!("unknown cohort: {cohort}")),
    };
    match clients {
        1 => Ok(vec![Workload::Insert(vec![1, large])]),
        n if n % 2 == 0 => Ok((0..n)
            .map(|i| Workload::Insert(vec![if i < n / 2 { 1 } else { large }]))
            .collect()),
        n => Err(format!(
            "{cohort} needs 1 or an even number of clients, got {n}"
        )),
    }
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
    // Optional readers, one per CPU, disjoint from the writers' CPUs.
    let reader_cpus = if args.iter().any(|arg| arg == "--reader-cpus") {
        parse_cpu_list(
            &option("--reader-cpus", args)?,
            "--reader-cpus",
            MAX_WORKERS,
        )?
    } else {
        Vec::new()
    };
    if reader_cpus.iter().any(|cpu| cpus.contains(cpu)) {
        return Err("--reader-cpus must be disjoint from --cpus".into());
    }
    let reader_config = ReaderConfig {
        gets: match option("--reader-gets", args) {
            Ok(v) => v.parse()?,
            Err(_) => READER_GETS,
        },
        scan: match option("--reader-scan", args) {
            Ok(v) => v.parse()?,
            Err(_) => READER_SCAN,
        },
    };
    let smoke_transactions = if args.iter().any(|arg| arg == "--smoke-transactions") {
        Some(option("--smoke-transactions", args)?.parse::<u64>()?)
    } else {
        None
    };
    if duration_ms != 2_000 && smoke_transactions.is_none() {
        return Err("timed trials require exactly 2000 ms".into());
    }
    if duration_ms == 0 || smoke_transactions == Some(0) {
        return Err("duration and smoke transactions must be positive".into());
    }
    let workloads = cohort_workloads(&cohort, cpus.len())?;
    let clients = workloads.len();
    let transfer = matches!(workloads[0], Workload::Transfer);
    let mut affinity: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(&affinity), &mut affinity) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if !(0..libc::CPU_SETSIZE as usize).all(|cpu| {
        let allowed = unsafe { libc::CPU_ISSET(cpu, &affinity) };
        allowed == (cpus.contains(&cpu) || reader_cpus.contains(&cpu))
    }) {
        return Err(format!(
            "process affinity must be exactly the requested CPUs {cpus:?} + readers {reader_cpus:?}"
        )
        .into());
    }
    if transfer && !reader_cpus.is_empty() {
        return Err("readers are defined for the insert cohorts only".into());
    }
    let db = create_database(&db_path)?;
    if transfer {
        seed_accounts(&db, TRANSFER_ACCOUNTS)?;
    }
    let writer = Writer::new(&db, variant)?;
    let barrier = Barrier::new(clients + reader_cpus.len() + 1);
    let start = OnceLock::new();
    let duration = Duration::from_millis(duration_ms);
    let mut perf = PerfControl::from_env()?;
    let perf_counted = perf.is_some();
    let done = AtomicBool::new(false);
    let (workers, readers, elapsed_ns, elapsed_tsc, process_cpu_ns) =
        thread::scope(|scope| -> Result<_, Box<dyn Error + Send + Sync>> {
            let handles: Vec<_> = workloads
                .iter()
                .enumerate()
                .map(|(id, workload)| {
                    let (writer, barrier, start, cpu) = (&writer, &barrier, &start, cpus[id]);
                    scope.spawn(move || {
                        worker(
                            id,
                            cpu,
                            workload,
                            seed,
                            durability,
                            writer,
                            barrier,
                            start,
                            duration,
                            smoke_transactions,
                        )
                    })
                })
                .collect();
            let reader_handles: Vec<_> = reader_cpus
                .iter()
                .enumerate()
                .map(|(id, &cpu)| {
                    let (db, barrier, start, done) = (&db, &barrier, &start, &done);
                    scope.spawn(move || {
                        reader(
                            id,
                            cpu,
                            db,
                            reader_config,
                            clients,
                            seed,
                            barrier,
                            start,
                            duration,
                            done,
                            smoke_transactions.is_some(),
                        )
                    })
                })
                .collect();
            barrier.wait();
            if let Some(perf) = perf.as_mut() {
                perf.send("enable")?;
            }
            let cpu_begin = cpu_time_ns()?;
            let tsc_begin = tsc();
            let begin = Instant::now();
            start.set(begin).map_err(|_| "start set twice")?;
            barrier.wait();
            let mut workers = Vec::with_capacity(clients);
            let mut failures = Vec::new();
            for handle in handles {
                match handle.join() {
                    Ok(Ok(result)) => workers.push(result),
                    Ok(Err(error)) => failures.push(error),
                    Err(_) => failures.push("worker panic".to_string()),
                }
            }
            done.store(true, Ordering::Release);
            let mut readers = Vec::with_capacity(reader_handles.len());
            for handle in reader_handles {
                match handle.join() {
                    Ok(Ok(result)) => readers.push(result),
                    Ok(Err(error)) => failures.push(error),
                    Err(_) => failures.push("reader panic".to_string()),
                }
            }
            let elapsed_ns = begin.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            let elapsed_tsc = tsc() - tsc_begin;
            let process_cpu_ns = cpu_time_ns()? - cpu_begin;
            if let Some(perf) = perf.as_mut() {
                perf.send("disable")?;
            }
            if !failures.is_empty() {
                return Err(failures.join("; ").into());
            }
            Ok((workers, readers, elapsed_ns, elapsed_tsc, process_cpu_ns))
        })?;
    // Reads stay outside the write gate: verification runs while it is still held.
    // Transfer: the fixed-insert table stays empty and balances are conserved.
    let live_insert = verify(&db, if transfer { &[] } else { &workers }, seed)?;
    let live_balances = if transfer {
        if live_insert != 0 {
            return Err("transfer cohort wrote the insert table".into());
        }
        Some(conserved_balances(&db, TRANSFER_ACCOUNTS)?)
    } else {
        None
    };
    let live = live_balances
        .as_ref()
        .map_or(live_insert, |b| b.len() as u64);
    drop(writer);
    drop(db);
    // Close/reopen checks normal process-close persistence, NOT crash/power-loss durability.
    let (reopened_exact, reopen_error) = match Database::open(&db_path) {
        Ok(reopened) => match &live_balances {
            None => match verify(&reopened, &workers, seed) {
                Ok(n) if n == live => (true, None),
                Ok(n) => (false, Some(format!("reopened {n}, live {live}"))),
                Err(error) => (false, Some(error)),
            },
            Some(balances) => match conserved_balances(&reopened, TRANSFER_ACCOUNTS) {
                Ok(reopened) if &reopened == balances => (true, None),
                Ok(_) => (
                    false,
                    Some("reopened balances differ from live balances".into()),
                ),
                Err(error) => (false, Some(error)),
            },
        },
        Err(error) => (false, Some(error.to_string())),
    };
    let transfer = live_balances.map(|_| TransferCheck {
        accounts: TRANSFER_ACCOUNTS,
        initial_balance: INITIAL_BALANCE,
        max_transfer: MAX_TRANSFER,
        total: TRANSFER_ACCOUNTS * INITIAL_BALANCE,
        conserved_live: true,
        reopened_identical: reopened_exact,
        committed: workers.iter().map(|w| w.completed_transactions).sum(),
        aborted: workers.iter().map(|w| w.aborted_transactions).sum(),
    });
    let output = Output {
        variant: variant.name().into(),
        cohort,
        clients,
        cpus,
        durability: durability_name,
        seed,
        smoke_transactions,
        duration_ms,
        window_ms: WINDOW_MS,
        max_records: MAX_RECORDS,
        max_records_per_worker: MAX_RECORDS_PER_WORKER,
        elapsed_ns,
        perf_counted,
        tsc_ticks_per_ns: elapsed_tsc as f64 / elapsed_ns as f64,
        process_cpu_ns,
        build: BUILD_INFO,
        workers,
        verified_live_records: live,
        reopened_exact,
        reopen_error,
        transfer,
        reader_cpus,
        reader_config,
        readers,
    };
    println!("{}", serde_json::to_string(&output)?);
    if output.durability == "immediate" && !output.reopened_exact {
        return Err("Immediate close/reopen verification failed".into());
    }
    // Transfer: conservation after reopen is required for both durabilities.
    if output.transfer.is_some() && !output.reopened_exact {
        return Err("transfer close/reopen verification failed".into());
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.iter().any(|arg| arg == "--build-info") {
        serde_json::to_string(&BUILD_INFO)
            .map(|info| println!("{info}"))
            .map_err(Into::into)
    } else if args.iter().any(|arg| arg == "--self-test") {
        selftest::run(&args)
    } else {
        trial(&args)
    };
    if let Err(error) = result {
        eprintln!("redb write trial failed: {error}");
        std::process::exit(1);
    }
}
