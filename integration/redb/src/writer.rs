//! The six write variants. Every variant performs the same narrow request:
//! 1..=64 new `u64 -> u64` records in one table, one durability mode, begin ->
//! inserts -> commit (explicit abort on a duplicate key).
//!
//! - `native`: upstream redb 3.1.0 public API, this file's `native_write`.
//! - `refactored`: patched redb's shared body under redb's own tracker lock.
//! - `bridge_mutex`/`mcs`/`fc`/`fc_pq`: patched redb's shared body submitted
//!   through `Bridge`; the lock backend is redb's write-serialisation mechanism.
use std::fmt;

use redb::TableDefinition;
#[cfg(feature = "native")]
use redb::{Database, Durability};

pub const TABLE: TableDefinition<u64, u64> = TableDefinition::new("writer_records");
pub const MAX_RECORDS_PER_REQUEST: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Native,
    Refactored,
    BridgeMutex,
    Mcs,
    Fc,
    FcPq,
}

impl Variant {
    pub fn parse(name: &str) -> Result<Self, String> {
        let variant = match name {
            "native" => Self::Native,
            "refactored" => Self::Refactored,
            "bridge_mutex" => Self::BridgeMutex,
            "mcs" => Self::Mcs,
            "fc" => Self::Fc,
            "fc_pq" => Self::FcPq,
            _ => return Err(format!("unknown variant: {name}")),
        };
        if (variant == Self::Native) != cfg!(feature = "native") {
            return Err(format!("variant {name} is not built into this binary"));
        }
        Ok(variant)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Refactored => "refactored",
            Self::BridgeMutex => "bridge_mutex",
            Self::Mcs => "mcs",
            Self::Fc => "fc",
            Self::FcPq => "fc_pq",
        }
    }

    pub fn delegated(self) -> bool {
        !matches!(self, Self::Native | Self::Refactored)
    }
}

#[derive(Debug)]
pub enum WriteError {
    /// Request outside the narrow shape; nothing executed.
    Rejected(String),
    /// Key already present; the transaction was aborted.
    Duplicate(u64),
    /// Test-hook build: injected error before commit; the transaction was aborted.
    #[cfg_attr(not(feature = "test_hooks"), allow(dead_code))]
    Injected,
    Other(String),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(reason) => write!(f, "rejected: {reason}"),
            Self::Duplicate(key) => write!(f, "duplicate key {key}; aborted"),
            Self::Injected => write!(f, "injected error; aborted"),
            Self::Other(error) => write!(f, "{error}"),
        }
    }
}

/// Transaction ID of the committed write when the variant exposes it (patched
/// variants); `None` for upstream native redb.
pub type WriteResult = Result<Option<u64>, WriteError>;

#[cfg(feature = "native")]
fn other<E: fmt::Display>(error: E) -> WriteError {
    WriteError::Other(error.to_string())
}

#[cfg(feature = "native")]
pub struct Writer<'db> {
    db: &'db Database,
}

#[cfg(feature = "native")]
impl<'db> Writer<'db> {
    pub fn new(db: &'db Database, variant: Variant) -> Result<Self, String> {
        assert_eq!(variant, Variant::Native);
        Ok(Self { db })
    }

    pub fn write(&self, records: &[(u64, u64)], durability: Durability) -> WriteResult {
        // Same request-shape rule as patched redb, so all variants see the same work.
        if records.is_empty() || records.len() > MAX_RECORDS_PER_REQUEST {
            return Err(WriteError::Rejected("record count outside 1..=64".into()));
        }
        native_write(self.db, records, durability).map(|()| None)
    }
}

/// Upstream-API sequence identical to patched redb's fixed-insert body.
#[cfg(feature = "native")]
fn native_write(db: &Database, records: &[(u64, u64)], durability: Durability) -> Result<(), WriteError> {
    let mut txn = db.begin_write().map_err(other)?;
    txn.set_durability(durability).map_err(other)?;
    {
        let mut table = txn.open_table(TABLE).map_err(other)?;
        for &(key, value) in records {
            if table.insert(key, value).map_err(other)?.is_some() {
                drop(table);
                txn.abort().map_err(other)?;
                return Err(WriteError::Duplicate(key));
            }
        }
    }
    txn.commit().map_err(other)?;
    Ok(())
}

#[cfg(feature = "patched")]
pub use patched::Writer;
#[cfg(feature = "test_hooks")]
pub use patched::remote_executions;

#[cfg(feature = "patched")]
mod patched {
    use std::cell::Cell;
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::PoisonError;

    use libdlock::dlock2::fc::FC;
    use libdlock::dlock2::fc_pq::{UsageNode, FCPQ};
    use libdlock::dlock2::mcs::RawMcsLock;
    use libdlock::dlock2::spinlock::DLock2Wrapper;
    use libdlock::dlock2::DLock2;
    use libdlock::spin_lock::RawSpinLock;
    use redb::dlock_private::{
        DelegatedCall, DelegatedWriteGate, FixedInsert, FixedInsertError, FixedInsertTarget,
        RawDelegatedCall,
    };
    use redb::{Database, Durability};

    use super::{Variant, WriteError, WriteResult, TABLE};

    pub enum Writer<'db> {
        Refactored(FixedInsertTarget<'db>),
        // Field order: the gate (delegated mode) ends before the bridge is dropped.
        Delegated {
            gate: DelegatedWriteGate<'db>,
            bridge: Bridge,
        },
    }

    impl<'db> Writer<'db> {
        pub fn new(db: &'db Database, variant: Variant) -> Result<Self, String> {
            let target = FixedInsertTarget::new(db, TABLE).map_err(|e| e.to_string())?;
            Ok(match variant {
                Variant::Native => unreachable!("native is a separate binary"),
                Variant::Refactored => Self::Refactored(target),
                _ => Self::Delegated {
                    gate: DelegatedWriteGate::enter(target).map_err(|e| e.to_string())?,
                    bridge: Bridge::new(variant),
                },
            })
        }

        pub fn write(&self, records: &[(u64, u64)], durability: Durability) -> WriteResult {
            let request = FixedInsert { records, durability };
            let outcome = match self {
                Self::Refactored(target) => target.execute_native(&request),
                Self::Delegated { gate, bridge } => {
                    gate.execute(&request, |call| bridge.submit(call))
                }
            };
            outcome.map(Some).map_err(|error| match error {
                FixedInsertError::Rejected(reason) => WriteError::Rejected(reason.into()),
                FixedInsertError::DuplicateKey(key) => WriteError::Duplicate(key),
                #[cfg(feature = "test_hooks")]
                FixedInsertError::Injected => WriteError::Injected,
                other => WriteError::Other(other.to_string()),
            })
        }
    }

    /// The only data a lock backend carries: a lifetime-erased delegated body.
    /// `call` is `Some` until the executor runs it exactly once.
    #[derive(Debug)]
    pub struct Request {
        call: Option<RawDelegatedCall>,
        #[cfg(feature = "test_hooks")]
        requester: std::thread::ThreadId,
    }

    /// Test-hook build: bodies executed on a thread other than their requester.
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
        unsafe { call.run() };
        request
    }

    type FnDelegate = fn(&mut (), Request) -> Request;
    type McsLock = DLock2Wrapper<(), Request, FnDelegate, RawMcsLock>;
    type FcLock = FC<(), Request, FnDelegate, RawSpinLock>;
    type PqLock =
        FCPQ<(), Request, BinaryHeap<Reverse<UsageNode<'static, Request>>>, FnDelegate, RawSpinLock>;

    enum Backend {
        // Same primitive type as redb's tracker lock, owned by the bridge: the
        // tracker's own Mutex cannot be borrowed because commit re-enters it.
        Mutex(std::sync::Mutex<()>),
        Mcs(McsLock),
        Fc(FcLock),
        FcPq(PqLock),
    }

    /// Synchronous submission bridge shared by `bridge_mutex`, `mcs`, `fc`, `fc_pq`.
    /// Mutex and MCS run the body on the requesting thread; FC/FC-PQ may run it
    /// on a combiner. Nested submissions are refused (the call is dropped unrun,
    /// so the gate reports `NotExecuted`); any panic in a backend aborts.
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
                Variant::BridgeMutex => Backend::Mutex(std::sync::Mutex::new(())),
                Variant::Mcs => Backend::Mcs(McsLock::new((), delegate)),
                Variant::Fc => Backend::Fc(FcLock::new((), delegate)),
                Variant::FcPq => Backend::FcPq(PqLock::new((), delegate)),
                Variant::Native | Variant::Refactored => unreachable!("not a bridge variant"),
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
                Backend::Fc(lock) => lock.lock(request),
                Backend::FcPq(lock) => lock.lock(request),
            }))
            .unwrap_or_else(|_| std::process::abort());
            if returned.call.is_some() {
                std::process::abort();
            }
        }
    }
}
