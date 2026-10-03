//! The seven write variants. Every variant runs the same caller closure
//! `FnOnce(&mut WriteTransaction) -> Result<R, WriteError>` as one whole write
//! transaction: begin -> closure -> commit on `Ok` / abort on `Err`. A panic in
//! the closure aborts the transaction and is re-raised on the requester.
//!
//! - `upstream`: upstream redb 3.1.0 public API, this file's `upstream_write`.
//! - `upstream_gate`: patched redb's shared body under redb's own tracker lock.
//! - `std_mutex`/`mcs`/`uscl`/`fc`/`fc_pq`: patched redb's shared body
//!   submitted through `Bridge`; the lock backend is redb's write-serialisation
//!   mechanism.
//!
//! The workload closures (`insert_body`, `transfer_body`) are shared by every
//! variant, so all variants perform identical database work.
//!
//! Service time (`service_time` feature): TSC ticks from `begin` returning to
//! commit/abort returning, read with `rdtscp` on the thread that runs the body
//! and returned to the requester with the outcome (`Written::service_tsc`).
//! Patched variants measure it inside redb's shared closure body; `upstream`
//! measures the identical span here.
use std::fmt;

#[cfg(feature = "upstream")]
use redb::Database;
use redb::{
    CommitError, Durability, ReadableTable, SavepointError, SetDurabilityError, StorageError,
    TableDefinition, TableError, TransactionError, WriteTransaction,
};

pub const TABLE: TableDefinition<u64, u64> = TableDefinition::new("writer_records");
pub const ACCOUNTS: TableDefinition<u64, u64> = TableDefinition::new("accounts");
pub const MAX_RECORDS_PER_REQUEST: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Upstream,
    UpstreamGate,
    StdMutex,
    Mcs,
    Uscl,
    Fc,
    FcPq,
}

impl Variant {
    pub fn parse(name: &str) -> Result<Self, String> {
        let variant = match name {
            "upstream" => Self::Upstream,
            "upstream_gate" => Self::UpstreamGate,
            "std_mutex" => Self::StdMutex,
            "mcs" => Self::Mcs,
            "uscl" => Self::Uscl,
            "fc" => Self::Fc,
            "fc_pq" => Self::FcPq,
            _ => return Err(format!("unknown variant: {name}")),
        };
        if (variant == Self::Upstream) != cfg!(feature = "upstream") {
            return Err(format!("variant {name} is not built into this binary"));
        }
        Ok(variant)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Upstream => "upstream",
            Self::UpstreamGate => "upstream_gate",
            Self::StdMutex => "std_mutex",
            Self::Mcs => "mcs",
            Self::Uscl => "uscl",
            Self::Fc => "fc",
            Self::FcPq => "fc_pq",
        }
    }

    pub fn delegated(self) -> bool {
        !matches!(self, Self::Upstream | Self::UpstreamGate)
    }

    /// FC and FC-PQ may run a closure on a combiner thread; Mutex, MCS and
    /// U-SCL run it on the requester.
    pub fn combining(self) -> bool {
        matches!(self, Self::Fc | Self::FcPq)
    }
}

/// The harness closures' error type. Any `Err` aborts the transaction.
#[derive(Debug)]
pub enum WriteError {
    /// Request outside the insert workload's shape; nothing was submitted.
    Rejected(String),
    /// Key already present; the transaction was aborted.
    Duplicate(u64),
    /// Transfer source balance below the amount; the transaction was aborted.
    Insufficient {
        account: u64,
        balance: u64,
        amount: u64,
    },
    /// A self-test closure's deliberate error; the transaction was aborted.
    Closure(String),
    /// The bridge refused the submission (nested submission); nothing ran.
    NotExecuted,
    Other(String),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(reason) => write!(f, "rejected: {reason}"),
            Self::Duplicate(key) => write!(f, "duplicate key {key}; aborted"),
            Self::Insufficient {
                account,
                balance,
                amount,
            } => {
                write!(f, "account {account} holds {balance} < {amount}; aborted")
            }
            Self::Closure(reason) => write!(f, "closure error: {reason}; aborted"),
            Self::NotExecuted => write!(f, "submission not executed by the bridge"),
            Self::Other(error) => write!(f, "{error}"),
        }
    }
}

macro_rules! redb_error {
    ($($source:ty),*) => {$(
        impl From<$source> for WriteError {
            fn from(error: $source) -> Self {
                Self::Other(error.to_string())
            }
        }
    )*};
}
redb_error!(
    StorageError,
    TableError,
    TransactionError,
    SetDurabilityError,
    CommitError,
    SavepointError
);

/// A completed write: the transaction ID when the variant exposes it (patched
/// variants; `None` for upstream redb), the closure's value and the
/// body's service time in TSC ticks (0 without the `service_time` feature).
#[derive(Debug)]
pub struct Written<R> {
    pub transaction_id: Option<u64>,
    pub value: R,
    pub service_tsc: u64,
}

pub type WriteResult<R> = Result<Written<R>, WriteError>;

/// Fixed-insert workload: 1..=64 new `u64 -> u64` records into `TABLE` with one
/// durability mode; a key that already exists aborts the whole transaction.
pub fn insert_body(
    records: &[(u64, u64)],
    durability: Durability,
) -> impl FnOnce(&mut WriteTransaction) -> Result<(), WriteError> + Send + '_ {
    move |txn| {
        txn.set_durability(durability)?;
        let mut table = txn.open_table(TABLE)?;
        for &(key, value) in records {
            if table.insert(key, value)?.is_some() {
                return Err(WriteError::Duplicate(key));
            }
        }
        Ok(())
    }
}

/// Transfer workload: read two accounts, move `amount` (2 reads + 2 updates).
/// The debit is written before the balance check, so an insufficient source
/// balance aborts a transaction that already holds a write (the abort must roll
/// it back). Returns the balances before the move.
pub fn transfer_body(
    from: u64,
    to: u64,
    amount: u64,
    durability: Durability,
) -> impl FnOnce(&mut WriteTransaction) -> Result<(u64, u64), WriteError> + Send {
    move |txn| {
        // A self-transfer would create money (both writes use pre-debit reads).
        if from == to {
            return Err(WriteError::Rejected("transfer to the same account".into()));
        }
        txn.set_durability(durability)?;
        let mut table = txn.open_table(ACCOUNTS)?;
        let missing = |account| WriteError::Other(format!("account {account} missing"));
        let source = table.get(from)?.ok_or_else(|| missing(from))?.value();
        let target = table.get(to)?.ok_or_else(|| missing(to))?.value();
        table.insert(from, source.saturating_sub(amount))?;
        if source < amount {
            return Err(WriteError::Insufficient {
                account: from,
                balance: source,
                amount,
            });
        }
        table.insert(to, target + amount)?;
        Ok((source, target))
    }
}

fn check_insert_shape(records: &[(u64, u64)]) -> Result<(), WriteError> {
    // Checked on the caller before submission: no lock, no transaction ID.
    if records.is_empty() || records.len() > MAX_RECORDS_PER_REQUEST {
        return Err(WriteError::Rejected("record count outside 1..=64".into()));
    }
    Ok(())
}

impl Writer<'_> {
    /// The fixed-insert workload through this variant.
    pub fn insert(&self, records: &[(u64, u64)], durability: Durability) -> WriteResult<()> {
        check_insert_shape(records)?;
        self.write(insert_body(records, durability))
    }
}

#[cfg(all(feature = "upstream", feature = "service_time"))]
#[inline(always)]
fn service_clock() -> u64 {
    let mut aux = 0_u32;
    // SAFETY: rdtscp only writes `aux`; this harness targets x86_64.
    unsafe { std::arch::x86_64::__rdtscp(&mut aux) }
}

#[cfg(all(feature = "upstream", not(feature = "service_time")))]
#[inline(always)]
fn service_clock() -> u64 {
    0
}

#[cfg(feature = "upstream")]
pub struct Writer<'db> {
    db: &'db Database,
}

#[cfg(feature = "upstream")]
impl<'db> Writer<'db> {
    pub fn new(db: &'db Database, variant: Variant) -> Result<Self, String> {
        assert_eq!(variant, Variant::Upstream);
        Ok(Self { db })
    }

    pub fn write<F, R>(&self, f: F) -> WriteResult<R>
    where
        F: FnOnce(&mut WriteTransaction) -> Result<R, WriteError> + Send,
        R: Send,
    {
        // Same service span as patched redb's body: begin returned -> commit/abort returned.
        let txn = self.db.begin_write()?;
        let admitted = service_clock();
        let outcome = upstream_write(txn, f);
        let service_tsc = service_clock().saturating_sub(admitted);
        outcome.map(|value| Written {
            transaction_id: None,
            value,
            service_tsc,
        })
    }

    /// FC-PQ fast-path hits; upstream has no FC-PQ.
    pub fn fast_path_hits(&self) -> Option<u64> {
        None
    }
}

/// Upstream-API sequence identical to patched redb's closure body after begin:
/// closure -> commit / abort, and an explicit abort after a caught closure panic
/// before the panic is resumed.
#[cfg(feature = "upstream")]
fn upstream_write<F, R>(mut txn: WriteTransaction, f: F) -> Result<R, WriteError>
where
    F: FnOnce(&mut WriteTransaction) -> Result<R, WriteError>,
{
    use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
    match catch_unwind(AssertUnwindSafe(|| f(&mut txn))) {
        Ok(Ok(value)) => {
            txn.commit()?;
            Ok(value)
        }
        Ok(Err(error)) => match txn.abort() {
            Ok(()) => Err(error),
            Err(abort) => Err(WriteError::Other(format!(
                "{error}; aborting also failed: {abort}"
            ))),
        },
        Err(payload) => {
            let _ = txn.abort();
            resume_unwind(payload)
        }
    }
}

#[cfg(feature = "test_hooks")]
pub use patched::remote_executions;
#[cfg(feature = "patched")]
pub use patched::Writer;

#[cfg(feature = "patched")]
mod patched {
    use std::cell::{Cell, UnsafeCell};
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    use std::mem::MaybeUninit;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::PoisonError;

    use libdlock::dlock2::fc::FC;
    use libdlock::dlock2::fc_pq::{UsageNode, FCPQ};
    use libdlock::dlock2::mcs::RawMcsLock;
    use libdlock::dlock2::spinlock::DLock2Wrapper;
    use libdlock::dlock2::DLock2;
    use libdlock::spin_lock::RawSpinLock;
    use libdlock::{
        fairlock_acquire, fairlock_bridge_destroy, fairlock_bridge_init, fairlock_release,
        fairlock_t, fairlock_thread_register,
    };
    use redb::dlock_private::{
        execute_upstream_gate_served, DelegatedCall, DelegatedWriteError, DelegatedWriteGate,
        RawDelegatedCall,
    };
    use redb::{Database, WriteTransaction};

    use super::{Variant, WriteError, WriteResult, Written};

    pub enum Writer<'db> {
        UpstreamGate(&'db Database),
        // Field order: the gate (delegated mode) ends before the bridge is dropped.
        Delegated {
            gate: DelegatedWriteGate<'db>,
            bridge: Bridge,
        },
    }

    impl<'db> Writer<'db> {
        pub fn new(db: &'db Database, variant: Variant) -> Result<Self, String> {
            Ok(match variant {
                Variant::Upstream => unreachable!("upstream is a separate binary"),
                Variant::UpstreamGate => Self::UpstreamGate(db),
                _ => Self::Delegated {
                    gate: DelegatedWriteGate::enter(db).map_err(|e| e.to_string())?,
                    bridge: Bridge::new(variant),
                },
            })
        }

        pub fn write<F, R>(&self, f: F) -> WriteResult<R>
        where
            F: FnOnce(&mut WriteTransaction) -> Result<R, WriteError> + Send,
            R: Send,
        {
            let served = match self {
                Self::UpstreamGate(db) => execute_upstream_gate_served(db, f),
                Self::Delegated { gate, bridge } => {
                    gate.execute_served(f, |call| bridge.submit(call))
                }
            };
            let service_tsc = served.service_tsc;
            served
                .outcome
                .map(|(id, value)| Written {
                    transaction_id: Some(id),
                    value,
                    service_tsc,
                })
                .map_err(|error| match error {
                    DelegatedWriteError::Write(error) => error,
                    DelegatedWriteError::NotExecuted => WriteError::NotExecuted,
                    other => WriteError::Other(other.to_string()),
                })
        }

        /// FC-PQ fast-path hits of the calling thread's requests (all of them,
        /// not only those credited to the timed window); `None` for other
        /// variants, before the thread's first request, or without
        /// `fcpq_fast_path_stat`.
        pub fn fast_path_hits(&self) -> Option<u64> {
            match self {
                Self::Delegated { bridge, .. } => bridge.fast_path_hits(),
                Self::UpstreamGate(_) => None,
            }
        }
    }

    /// The only data a lock backend carries: a lifetime-erased delegated call.
    /// `call` is `Some` until the executor runs it exactly once.
    #[derive(Debug)]
    pub struct Request {
        call: Option<RawDelegatedCall>,
        #[cfg(feature = "test_hooks")]
        requester: std::thread::ThreadId,
    }

    /// Test-hook build: calls executed on a thread other than their requester.
    #[cfg(feature = "test_hooks")]
    static REMOTE_EXECUTIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    #[cfg(feature = "test_hooks")]
    pub fn remote_executions() -> u64 {
        REMOTE_EXECUTIONS.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn delegate(_: &mut (), mut request: Request) -> Request {
        let _depth = CallbackDepthGuard::enter();
        #[cfg(feature = "test_hooks")]
        if request.requester != std::thread::current().id() {
            REMOTE_EXECUTIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let call = request.call.take().unwrap_or_else(|| std::process::abort());
        // SAFETY: the requester is blocked in Bridge::submit until this returns.
        // Closure panics are caught inside the call; nothing unwinds out of it.
        unsafe { call.run() };
        request
    }

    type FnDelegate = fn(&mut (), Request) -> Request;
    type McsLock = DLock2Wrapper<(), Request, FnDelegate, RawMcsLock>;
    type FcLock = FC<(), Request, FnDelegate, RawSpinLock>;
    type PqLock = FCPQ<
        (),
        Request,
        BinaryHeap<Reverse<UsageNode<'static, Request>>>,
        FnDelegate,
        RawSpinLock,
    >;

    /// U-SCL (fairlock), as in crates/upscaledb-bridge: the C lock treats
    /// `&lock->qnext` as an embedded queue node and keeps per-thread state in
    /// pthread TLS, so it lives at a stable boxed address and is initialised in
    /// bridge mode (TLS destructor, owner pointer). The closure runs on the
    /// requester. U-SCL's own waiting (futex queue, nanosleep ban) is part of the
    /// algorithm and is left as upstream implements it.
    struct UsclLock(Box<UnsafeCell<MaybeUninit<fairlock_t>>>);

    // SAFETY: the C implementation synchronises every access to the lock
    // object; the Box keeps its address stable for the bridge's lifetime.
    unsafe impl Send for UsclLock {}
    unsafe impl Sync for UsclLock {}

    impl UsclLock {
        fn new() -> Self {
            let storage = Box::new(UnsafeCell::new(MaybeUninit::<fairlock_t>::uninit()));
            if unsafe { fairlock_bridge_init(storage.get().cast()) } != 0 {
                std::process::abort();
            }
            Self(storage)
        }

        fn ptr(&self) -> *mut fairlock_t {
            self.0.get().cast()
        }

        fn run(&self, request: Request) -> Request {
            // SAFETY: initialised in `new`; registration is idempotent per
            // live thread and key. Equal weights, independent of nice.
            unsafe {
                fairlock_thread_register(self.ptr(), 1024);
                fairlock_acquire(self.ptr());
            }
            let request = delegate(&mut (), request);
            unsafe { fairlock_release(self.ptr()) };
            request
        }
    }

    impl Drop for UsclLock {
        fn drop(&mut self) {
            // Every submitting thread has been joined (pthread_join), so its
            // TLS destructor has already run; this thread's entry is freed here.
            if unsafe { fairlock_bridge_destroy(self.ptr()) } != 0 {
                std::process::abort();
            }
        }
    }

    enum Backend {
        // Same primitive type as redb's tracker lock, owned by the bridge: the
        // tracker's own Mutex cannot be borrowed because commit re-enters it.
        Mutex(std::sync::Mutex<()>),
        Mcs(McsLock),
        Uscl(UsclLock),
        Fc(FcLock),
        FcPq(PqLock),
    }

    /// Synchronous submission bridge shared by `std_mutex`, `mcs`, `uscl`,
    /// `fc`, `fc_pq`. Mutex, MCS and U-SCL run the closure on the requesting
    /// thread; FC/FC-PQ may run it on a combiner. Nested submissions are refused
    /// (the call is dropped unrun, so the gate reports `NotExecuted`); any panic
    /// in a backend aborts.
    pub struct Bridge {
        backend: Backend,
    }

    thread_local! {
        static BRIDGE_DEPTH: Cell<bool> = const { Cell::new(false) };
    }

    struct DepthGuard;
    impl DepthGuard {
        fn enter() -> Option<Self> {
            BRIDGE_DEPTH.with(|depth| (!depth.replace(true)).then_some(Self))
        }
    }
    impl Drop for DepthGuard {
        fn drop(&mut self) {
            BRIDGE_DEPTH.with(|depth| depth.set(false));
        }
    }

    struct CallbackDepthGuard(bool);
    impl CallbackDepthGuard {
        fn enter() -> Self {
            Self(BRIDGE_DEPTH.with(|depth| depth.replace(true)))
        }
    }
    impl Drop for CallbackDepthGuard {
        fn drop(&mut self) {
            BRIDGE_DEPTH.with(|depth| depth.set(self.0));
        }
    }

    impl Bridge {
        fn new(variant: Variant) -> Self {
            let delegate = delegate as FnDelegate;
            let backend = match variant {
                Variant::StdMutex => Backend::Mutex(std::sync::Mutex::new(())),
                Variant::Mcs => Backend::Mcs(McsLock::new((), delegate)),
                Variant::Uscl => Backend::Uscl(UsclLock::new()),
                Variant::Fc => Backend::Fc(FcLock::new((), delegate)),
                Variant::FcPq => Backend::FcPq(PqLock::new((), delegate)),
                Variant::Upstream | Variant::UpstreamGate => unreachable!("not a bridge variant"),
            };
            Self { backend }
        }

        fn submit(&self, call: DelegatedCall<'_>) {
            let Some(_depth) = DepthGuard::enter() else {
                return;
            };
            // SAFETY: every backend below runs `delegate` (which runs the call)
            // before `lock` returns; a returned unrun call aborts below.
            let request = Request {
                call: Some(unsafe { call.into_raw() }),
                #[cfg(feature = "test_hooks")]
                requester: std::thread::current().id(),
            };
            let returned = catch_unwind(AssertUnwindSafe(|| match &self.backend {
                Backend::Mutex(mutex) => {
                    let _guard = mutex.lock().unwrap_or_else(PoisonError::into_inner);
                    delegate(&mut (), request)
                }
                Backend::Mcs(lock) => lock.lock(request),
                Backend::Uscl(lock) => lock.run(request),
                Backend::Fc(lock) => lock.lock(request),
                Backend::FcPq(lock) => lock.lock(request),
            }))
            .unwrap_or_else(|_| std::process::abort());
            if returned.call.is_some() {
                std::process::abort();
            }
        }

        fn fast_path_hits(&self) -> Option<u64> {
            match &self.backend {
                #[cfg(feature = "fcpq_fast_path_stat")]
                Backend::FcPq(lock) => lock.get_fast_path_hits(),
                _ => None,
            }
        }
    }
}
