//! Synchronous FFI adapter for the restricted UpScaleDB experiment.
//!
//! The C caller owns the mutex (in control mode), handle usage, and every
//! context/result/metrics allocation. All possible ABI callers, including USCL
//! TLS teardown, must finish and join before exclusive destruction.

use std::cell::{Cell, UnsafeCell};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
#[cfg(feature = "profile")]
use std::sync::atomic::{AtomicU64, Ordering};

use libdlock::dlock2::cfl::RawCflLock;
use libdlock::dlock2::clh::RawClhLock;
use libdlock::dlock2::fc::FC;
use libdlock::dlock2::fc_pq::{UsageNode, FCPQ};
use libdlock::dlock2::mcs::RawMcsLock;
use libdlock::dlock2::spinlock::DLock2Wrapper;
use libdlock::dlock2::ticket::RawTicketLock;
use libdlock::dlock2::DLock2;
use libdlock::spin_lock::RawSpinLock;
use libdlock::{
    fairlock_acquire, fairlock_bridge_destroy, fairlock_bridge_init, fairlock_release, fairlock_t,
    fairlock_thread_register,
};

#[cfg(feature = "profile")]
use std::arch::x86_64::__rdtscp;
#[cfg(feature = "profile")]
use std::time::Instant;
#[cfg(feature = "profile")]
use thread_local::ThreadLocal;

const OK: i32 = 0;
const INVALID: i32 = 1;
const REENTRANT: i32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct DlockRequestMetrics {
    pub service_tsc_ticks: u64,
    pub service_ns: u64,
    pub enabled: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct DlockThreadMetrics {
    pub executed_callbacks: u64,
    pub executed_service_tsc_ticks: u64,
    pub executed_service_ns: u64,
    pub combiner_pass_tsc_ticks: u64,
    pub enabled: u32,
    pub has_combiner_pass_ticks: u32,
}

/// Global counters are profile-only instrumentation, not lifecycle state.
/// In non-profile builds enabled and all counters are zero.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct DlockGlobalMetrics {
    pub accepted_calls: u64,
    pub completed_calls: u64,
    pub rejected_reentrant: u64,
    pub peak_inflight_calls: u64,
    pub active_calls: u64,
    pub enabled: u32,
    pub reserved: u32,
}

pub type BridgeCallback = unsafe extern "C" fn(*mut c_void);

// SAFETY: This descriptor is moved between workers only while the submitting
// worker is blocked inside execute. It never outlives that call. C guarantees
// that context and all reachable inputs/outputs (including optional metrics)
// remain live and exclusively writable by the executing callback until the
// request returns, and does not touch them while pending. The callback may run
// on *another* physical worker; it must not retain pointers, unwind, longjmp,
// cancel the thread or call back into a bridge submission. FC/FC-PQ publish
// callback effects with release completion and the waiter acquires completion
// before returning; pthread unlock supplies the control-mode handoff.
#[derive(Clone, Copy, Debug)]
struct Request {
    callback: Option<BridgeCallback>,
    context: *mut c_void,
    #[cfg(feature = "profile")]
    metrics: *mut DlockRequestMetrics,
    #[cfg(feature = "profile")]
    profile: *const Profile,
    #[cfg(feature = "test-hooks")]
    panic_in_delegate: bool,
}
unsafe impl Send for Request {}
// SAFETY: libdlock's FC-PQ queue may retain shared references to publication
// nodes containing a Request. Those references only observe this immutable
// descriptor; they never dereference its raw C pointers. The sole callback
// invocation receives the descriptor by value after publication, while the
// caller exclusively owns reachable storage until synchronous completion.
unsafe impl Sync for Request {}

#[cfg(feature = "profile")]
#[derive(Clone, Copy, Default)]
struct WorkerCounters {
    owner: Option<std::thread::ThreadId>,
    pass_baseline: u64,
    callbacks: u64,
    ticks: u64,
    ns: u64,
}

#[cfg(feature = "profile")]
struct Profile {
    // One allocation per *handle* and physical worker; never a process-wide
    // TLS aggregate that would include warmup or another handle's work.
    workers: ThreadLocal<Cell<WorkerCounters>>,
    accepted: AtomicU64,
    completed: AtomicU64,
    reentrant: AtomicU64,
    peak: AtomicU64,
    active: AtomicU64,
}

#[cfg(feature = "profile")]
impl Profile {
    fn new() -> Self {
        Self {
            workers: ThreadLocal::new(),
            accepted: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            reentrant: AtomicU64::new(0),
            active: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        }
    }

    fn register(&self, baseline: u64) {
        let owner = std::thread::current().id();
        let local = self.workers.get_or(|| Cell::new(WorkerCounters::default()));
        if local.get().owner != Some(owner) {
            // ThreadLocal registration slots can be recycled after thread
            // exit. Do not attribute the former worker's callbacks or the
            // lock's cumulative combining ticks to this new physical worker.
            local.set(WorkerCounters {
                owner: Some(owner),
                pass_baseline: baseline,
                ..WorkerCounters::default()
            });
        }
    }

    fn record(&self, ticks: u64, ns: u64) {
        let local = self.workers.get().unwrap_or_else(|| std::process::abort());
        let mut counters = local.get();
        if counters.owner != Some(std::thread::current().id()) {
            std::process::abort();
        }
        counters.callbacks = counters.callbacks.saturating_add(1);
        counters.ticks = counters.ticks.saturating_add(ticks);
        counters.ns = counters.ns.saturating_add(ns);
        local.set(counters);
    }
}

fn delegate(_: &mut (), request: Request) -> Request {
    // Explicitly cover the physical executor even if it differs from the
    // requesting worker. The submitting worker keeps its own guard until return.
    let _callback_depth = CallbackDepthGuard::enter();
    #[cfg(feature = "test-hooks")]
    if request.panic_in_delegate {
        panic!("injected panic inside protected bridge delegate");
    }
    // SAFETY: create/execute validated the callback and the caller retains all
    // context storage until this synchronous request has completed.
    let callback = request.callback.unwrap_or_else(|| std::process::abort());
    #[cfg(feature = "profile")]
    {
        let start_ns = Instant::now();
        let mut aux = 0;
        // SAFETY: the integration is x86_64, and aux is writable.
        let start_tsc = unsafe { __rdtscp(&mut aux) };
        unsafe { callback(request.context) };
        let end_tsc = unsafe { __rdtscp(&mut aux) };
        let elapsed_ns = start_ns.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let ticks = end_tsc.wrapping_sub(start_tsc);
        // SAFETY: the caller keeps the handle and optional request metrics
        // alive until synchronous completion; destroy requires joined callers.
        unsafe {
            let profile = &*request.profile;
            profile.record(ticks, elapsed_ns);
            if !request.metrics.is_null() {
                *request.metrics = DlockRequestMetrics {
                    service_tsc_ticks: ticks,
                    service_ns: elapsed_ns,
                    enabled: 1,
                    reserved: 0,
                };
            }
        }
    }
    #[cfg(not(feature = "profile"))]
    unsafe {
        callback(request.context);
    }
    request
}

type FnDelegate = fn(&mut (), Request) -> Request;
type FcLock = FC<(), Request, FnDelegate, RawSpinLock>;
type PqLock =
    FCPQ<(), Request, BinaryHeap<Reverse<UsageNode<'static, Request>>>, FnDelegate, RawSpinLock>;
type CflLock = DLock2Wrapper<(), Request, FnDelegate, RawCflLock>;
type SpinLock = DLock2Wrapper<(), Request, FnDelegate, RawSpinLock>;
type McsLock = DLock2Wrapper<(), Request, FnDelegate, RawMcsLock>;
type TicketLock = DLock2Wrapper<(), Request, FnDelegate, RawTicketLock>;
type ClhLock = DLock2Wrapper<(), Request, FnDelegate, RawClhLock>;

// The C implementation treats &lock->qnext as an embedded queue node and
// stores lock pointers in pthread TLS. Box keeps the address stable; UnsafeCell
// permits C to mutate the object from concurrent shared bridge references.
// C owns the locking discipline, and bridge destroy requires all workers joined.
struct UsclLock(Box<UnsafeCell<MaybeUninit<fairlock_t>>>);

impl UsclLock {
    fn new() -> Option<Self> {
        let storage = Box::new(UnsafeCell::new(MaybeUninit::<fairlock_t>::uninit()));
        if unsafe { fairlock_bridge_init(storage.get().cast()) } != 0 {
            return None;
        }
        Some(Self(storage))
    }

    fn ptr(&self) -> *mut fairlock_t {
        self.0.get().cast()
    }

    fn run(&self, request: Request) {
        unsafe {
            // Equal positive weights, independent of nice / getpriority(0).
            // C TLS registration is idempotent for each live worker and key.
            fairlock_thread_register(self.ptr(), 1024);
            fairlock_acquire(self.ptr());
            delegate(&mut (), request);
            fairlock_release(self.ptr());
        }
    }
}

impl Drop for UsclLock {
    fn drop(&mut self) {
        // Bridge destroy requires joins of every submitting worker:
        // pthread TLS destructors on those workers have finished first.
        if unsafe { fairlock_bridge_destroy(self.ptr()) } != 0 {
            std::process::abort();
        }
    }
}

enum Backend {
    // The original, initialized boost::mutex native_handle, never owned here.
    Mutex(*mut libc::pthread_mutex_t),
    Fc(FcLock),
    Pq(PqLock),
    Uscl(UsclLock),
    CflLocal(CflLock),
    Spin(SpinLock),
    Mcs(McsLock),
    Ticket(TicketLock),
    Clh(ClhLock),
}

#[cfg(feature = "profile")]
impl Backend {
    fn pass_ticks(&self) -> u64 {
        match self {
            Backend::Mutex(_)
            | Backend::Uscl(_)
            | Backend::CflLocal(_)
            | Backend::Spin(_)
            | Backend::Mcs(_)
            | Backend::Ticket(_)
            | Backend::Clh(_) => 0,
            Backend::Fc(lock) => lock.get_combine_time().unwrap_or(0),
            Backend::Pq(lock) => lock.get_combine_time().unwrap_or(0),
        }
    }
}

#[repr(C)]
pub struct DlockBridge {
    backend: Backend,
    #[cfg(feature = "profile")]
    profile: Box<Profile>,
}

// SAFETY: the caller joins every possible ABI caller before exclusive destroy.
// The backend serializes delegates; the borrowed mutex lives until destroy.
unsafe impl Send for DlockBridge {}
unsafe impl Sync for DlockBridge {}

thread_local! {
    // Depth, rather than handle identity: a callback on a remote combiner can
    // create a cross-handle lock cycle just as easily as same-handle recursion.
    // Also remains set while this worker is waiting on another combiner.
    static BRIDGE_DEPTH: Cell<bool> = const { Cell::new(false) };
}

struct DepthGuard;
impl DepthGuard {
    fn enter() -> Option<Self> {
        BRIDGE_DEPTH.with(|depth| {
            if depth.get() {
                None
            } else {
                depth.set(true);
                Some(Self)
            }
        })
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

impl DlockBridge {
    fn run(&self, request: Request) {
        // A panic may leave the FC combiner lock and its node/queue poisoned.
        // Never release the handle for reuse after it: abort immediately. Rust
        // extern "C" entry points themselves also prohibit escaping unwinds.
        if catch_unwind(AssertUnwindSafe(|| match &self.backend {
            Backend::Mutex(mutex) => {
                // SAFETY: the caller lends this initialized mutex until destroy.
                // Pthread lock/unlock must be paired on this physical worker.
                unsafe {
                    if libc::pthread_mutex_lock(*mutex) != 0 {
                        std::process::abort();
                    }
                    delegate(&mut (), request);
                    if libc::pthread_mutex_unlock(*mutex) != 0 {
                        std::process::abort();
                    }
                }
            }
            Backend::Fc(lock) => {
                lock.lock(request);
            }
            Backend::Pq(lock) => {
                lock.lock(request);
            }
            Backend::Uscl(lock) => lock.run(request),
            Backend::CflLocal(lock) => {
                lock.lock(request);
            }
            Backend::Spin(lock) => {
                lock.lock(request);
            }
            Backend::Mcs(lock) => {
                lock.lock(request);
            }
            Backend::Ticket(lock) => {
                lock.lock(request);
            }
            Backend::Clh(lock) => {
                lock.lock(request);
            }
        }))
        .is_err()
        {
            std::process::abort();
        }
    }

    fn execute(
        &self,
        callback: Option<BridgeCallback>,
        context: *mut c_void,
        metrics: *mut DlockRequestMetrics,
        panic_in_delegate: bool,
    ) -> i32 {
        // This check precedes *any* backend ThreadLocal publication-node access.
        let _depth = match DepthGuard::enter() {
            Some(depth) => depth,
            None => {
                #[cfg(feature = "profile")]
                self.profile.reentrant.fetch_add(1, Ordering::Relaxed);
                return REENTRANT;
            }
        };
        if !metrics.is_null() {
            unsafe {
                *metrics = DlockRequestMetrics {
                    enabled: u32::from(cfg!(feature = "profile")),
                    ..DlockRequestMetrics::default()
                };
            }
        }
        // Profile-only in-flight instrumentation: never used to gate calls,
        // synchronize shutdown, or protect handle lifetime.
        #[cfg(feature = "profile")]
        {
            self.profile.accepted.fetch_add(1, Ordering::Relaxed);
            let active = self.profile.active.fetch_add(1, Ordering::Relaxed) + 1;
            self.profile.peak.fetch_max(active, Ordering::Relaxed);
        }
        #[cfg(feature = "profile")]
        self.profile.register(self.backend.pass_ticks());
        let request = Request {
            callback,
            context,
            #[cfg(feature = "profile")]
            metrics,
            #[cfg(feature = "profile")]
            profile: self.profile.as_ref() as *const Profile,
            #[cfg(feature = "test-hooks")]
            panic_in_delegate,
        };
        #[cfg(not(feature = "profile"))]
        let _ = metrics;
        #[cfg(not(feature = "test-hooks"))]
        let _ = panic_in_delegate;
        self.run(request);
        #[cfg(feature = "profile")]
        {
            self.profile.completed.fetch_add(1, Ordering::Relaxed);
            self.profile.active.fetch_sub(1, Ordering::Relaxed);
        }
        OK
    }
}

/// Create a bridge. The caller must not publish the handle before initialization
/// and must join all possible ABI callers before destroying it.
#[no_mangle]
pub unsafe extern "C" fn dlock_bridge_create(
    kind: u32,
    native_mutex: *mut c_void,
    out_handle: *mut *mut DlockBridge,
) -> i32 {
    if out_handle.is_null() {
        return INVALID;
    }
    // SAFETY: out_handle is caller-provided writable pointer per the C ABI.
    unsafe { *out_handle = ptr::null_mut() };
    let backend = match kind {
        0 if !native_mutex.is_null() => Backend::Mutex(native_mutex.cast()),
        1 => Backend::Fc(FcLock::new((), delegate as FnDelegate)),
        2 => Backend::Pq(PqLock::new((), delegate as FnDelegate)),
        3 => match UsclLock::new() {
            Some(lock) => Backend::Uscl(lock),
            None => return INVALID,
        },
        4 => Backend::CflLocal(CflLock::new((), delegate as FnDelegate)),
        5 => Backend::Spin(SpinLock::new((), delegate as FnDelegate)),
        6 => Backend::Mcs(McsLock::new((), delegate as FnDelegate)),
        7 => Backend::Ticket(TicketLock::new((), delegate as FnDelegate)),
        8 => Backend::Clh(ClhLock::new((), delegate as FnDelegate)),
        _ => return INVALID,
    };
    let bridge = Box::new(DlockBridge {
        backend,
        #[cfg(feature = "profile")]
        profile: Box::new(Profile::new()),
    });
    unsafe { *out_handle = Box::into_raw(bridge) };
    OK
}

/// On success callback effects and caller-owned output are visible on return.
/// Null metrics is allowed; nonnull metrics is exclusive and writable.
/// The caller holds the handle alive until return and joins every possible
/// caller before destruction. Callbacks must not unwind across this ABI.
#[no_mangle]
pub unsafe extern "C" fn dlock_bridge_execute(
    handle: *mut DlockBridge,
    callback: Option<BridgeCallback>,
    context: *mut c_void,
    metrics: *mut DlockRequestMetrics,
) -> i32 {
    if handle.is_null() || callback.is_none() {
        return INVALID;
    }
    unsafe { &*handle }.execute(callback, context, metrics, false)
}

/// Destroy only after every possible ABI caller has joined (including USCL
/// TLS teardown). Concurrent ABI access or any future use is undefined.
#[no_mangle]
pub unsafe extern "C" fn dlock_bridge_destroy(handle: *mut DlockBridge) -> i32 {
    if handle.is_null() {
        return INVALID;
    }
    // No concurrent ABI calls or future uses of this pointer are allowed:
    // the caller must join every worker, including USCL pthread TLS cleanup.
    // Reject same-thread callback reentry before freeing the active handle.
    if BRIDGE_DEPTH.with(Cell::get) {
        return REENTRANT;
    }
    unsafe { drop(Box::from_raw(handle)) };
    OK
}

#[no_mangle]
pub unsafe extern "C" fn dlock_bridge_get_thread_metrics(
    handle: *mut DlockBridge,
    out_metrics: *mut DlockThreadMetrics,
) -> i32 {
    if handle.is_null() || out_metrics.is_null() {
        return INVALID;
    }
    let bridge = unsafe { &*handle };
    #[cfg(feature = "profile")]
    let mut snapshot = DlockThreadMetrics::default();
    #[cfg(not(feature = "profile"))]
    let snapshot = DlockThreadMetrics::default();
    #[cfg(feature = "profile")]
    {
        snapshot.enabled = 1;
        snapshot.has_combiner_pass_ticks =
            u32::from(matches!(&bridge.backend, Backend::Fc(_) | Backend::Pq(_)));
        if let Some(local) = bridge.profile.workers.get() {
            let counters = local.get();
            if counters.owner == Some(std::thread::current().id()) {
                snapshot.executed_callbacks = counters.callbacks;
                snapshot.executed_service_tsc_ticks = counters.ticks;
                snapshot.executed_service_ns = counters.ns;
                snapshot.combiner_pass_tsc_ticks = bridge
                    .backend
                    .pass_ticks()
                    .saturating_sub(counters.pass_baseline);
            }
        }
    }
    #[cfg(not(feature = "profile"))]
    let _ = bridge;
    unsafe { *out_metrics = snapshot };
    OK
}

#[no_mangle]
pub unsafe extern "C" fn dlock_bridge_get_global_metrics(
    handle: *mut DlockBridge,
    out_metrics: *mut DlockGlobalMetrics,
) -> i32 {
    if handle.is_null() || out_metrics.is_null() {
        return INVALID;
    }
    let bridge = unsafe { &*handle };
    #[cfg(feature = "profile")]
    let mut snapshot = DlockGlobalMetrics::default();
    #[cfg(not(feature = "profile"))]
    let snapshot = DlockGlobalMetrics::default();
    #[cfg(feature = "profile")]
    {
        snapshot.enabled = 1;
        snapshot.accepted_calls = bridge.profile.accepted.load(Ordering::Relaxed);
        snapshot.completed_calls = bridge.profile.completed.load(Ordering::Relaxed);
        snapshot.rejected_reentrant = bridge.profile.reentrant.load(Ordering::Relaxed);
        snapshot.peak_inflight_calls = bridge.profile.peak.load(Ordering::Relaxed);
        snapshot.active_calls = bridge.profile.active.load(Ordering::Relaxed);
    }
    #[cfg(not(feature = "profile"))]
    let _ = bridge;
    unsafe { *out_metrics = snapshot };
    OK
}

#[cfg(feature = "test-hooks")]
#[no_mangle]
pub unsafe extern "C" fn dlock_bridge_test_panic(handle: *mut DlockBridge) -> i32 {
    if handle.is_null() {
        return INVALID;
    }
    unsafe { &*handle }.execute(None, ptr::null_mut(), ptr::null_mut(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn create(kind: u32, mutex: *mut c_void) -> *mut DlockBridge {
        let mut bridge = ptr::null_mut();
        assert_eq!(unsafe { dlock_bridge_create(kind, mutex, &mut bridge) }, OK);
        assert!(!bridge.is_null());
        bridge
    }

    fn close(bridge: *mut DlockBridge) {
        // All workers that can use the handle have been joined by the caller.
        assert_eq!(unsafe { dlock_bridge_destroy(bridge) }, OK);
    }

    struct Answer {
        input: u64,
        output: u64,
        requester: thread::ThreadId,
        remote: Arc<AtomicU64>,
    }

    unsafe extern "C" fn answer_callback(raw: *mut c_void) {
        let answer = unsafe { &mut *raw.cast::<Answer>() };
        if thread::current().id() != answer.requester {
            answer.remote.fetch_add(1, AtomicOrdering::Relaxed);
        }
        answer.output = answer.input ^ 0x61931abc;
        thread::yield_now();
    }

    unsafe extern "C" fn noop_callback(_: *mut c_void) {}

    #[test]
    fn abi_validation_mutex_borrow_and_synchronous_results() {
        for kind in [9, 42, u32::MAX] {
            let mut output = ptr::dangling_mut::<DlockBridge>();
            assert_eq!(
                unsafe { dlock_bridge_create(kind, ptr::null_mut(), &mut output) },
                INVALID
            );
            assert!(output.is_null());
        }
        let mut output = ptr::null_mut();
        assert_eq!(
            unsafe { dlock_bridge_create(0, ptr::null_mut(), &mut output) },
            INVALID
        );
        assert_eq!(
            unsafe { dlock_bridge_create(1, ptr::null_mut(), ptr::null_mut()) },
            INVALID
        );
        assert_eq!(
            unsafe {
                dlock_bridge_execute(
                    ptr::null_mut(),
                    Some(answer_callback),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            },
            INVALID
        );
        assert_eq!(unsafe { dlock_bridge_destroy(ptr::null_mut()) }, INVALID);
        let mut thread_metrics = DlockThreadMetrics::default();
        let mut global_metrics = DlockGlobalMetrics::default();
        assert_eq!(
            unsafe { dlock_bridge_get_thread_metrics(ptr::null_mut(), &mut thread_metrics) },
            INVALID
        );
        assert_eq!(
            unsafe { dlock_bridge_get_global_metrics(ptr::null_mut(), &mut global_metrics) },
            INVALID
        );
        let bridge = create(1, ptr::null_mut());
        assert_eq!(
            unsafe { dlock_bridge_execute(bridge, None, ptr::null_mut(), ptr::null_mut()) },
            INVALID
        );
        assert_eq!(
            unsafe { dlock_bridge_get_thread_metrics(bridge, ptr::null_mut()) },
            INVALID
        );
        assert_eq!(
            unsafe { dlock_bridge_get_global_metrics(bridge, ptr::null_mut()) },
            INVALID
        );
        close(bridge);

        // Actual original pthread mutex remains owned by the caller, not the bridge.
        let mut mutex = Box::new(unsafe { std::mem::zeroed::<libc::pthread_mutex_t>() });
        assert_eq!(
            unsafe { libc::pthread_mutex_init(&mut *mutex, ptr::null()) },
            0
        );
        let bridge = create(0, (&mut *mutex as *mut libc::pthread_mutex_t).cast());
        let mut data = Answer {
            input: 73,
            output: 0,
            requester: thread::current().id(),
            remote: Arc::new(AtomicU64::new(0)),
        };
        let mut request = DlockRequestMetrics::default();
        assert_eq!(
            unsafe {
                dlock_bridge_execute(
                    bridge,
                    Some(answer_callback),
                    (&mut data as *mut Answer).cast(),
                    &mut request,
                )
            },
            OK
        );
        assert_eq!(data.output, 73 ^ 0x61931abc);
        assert_eq!(request.enabled, u32::from(cfg!(feature = "profile")));
        close(bridge);
        assert_eq!(unsafe { libc::pthread_mutex_trylock(&mut *mutex) }, 0);
        assert_eq!(unsafe { libc::pthread_mutex_unlock(&mut *mutex) }, 0);
        assert_eq!(unsafe { libc::pthread_mutex_destroy(&mut *mutex) }, 0);
    }

    struct Nested {
        first: *mut DlockBridge,
        second: *mut DlockBridge,
        self_status: i32,
        cross_status: i32,
        destroy_status: i32,
    }
    unsafe extern "C" fn nested_callback(raw: *mut c_void) {
        let context = unsafe { &mut *raw.cast::<Nested>() };
        context.self_status = unsafe {
            dlock_bridge_execute(
                context.first,
                Some(noop_callback),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        context.cross_status = unsafe {
            dlock_bridge_execute(
                context.second,
                Some(noop_callback),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        context.destroy_status = unsafe { dlock_bridge_destroy(context.first) };
    }

    #[test]
    fn nested_same_and_cross_handle_submissions_are_rejected() {
        for kind in 0..=8 {
            let mut mutex = Box::new(unsafe { std::mem::zeroed::<libc::pthread_mutex_t>() });
            assert_eq!(
                unsafe { libc::pthread_mutex_init(&mut *mutex, ptr::null()) },
                0
            );
            let first = create(kind, (&mut *mutex as *mut libc::pthread_mutex_t).cast());
            let second = create(1, ptr::null_mut());
            let mut nested = Nested {
                first,
                second,
                self_status: 0,
                cross_status: 0,
                destroy_status: 0,
            };
            assert_eq!(
                unsafe {
                    dlock_bridge_execute(
                        first,
                        Some(nested_callback),
                        (&mut nested as *mut Nested).cast(),
                        ptr::null_mut(),
                    )
                },
                OK
            );
            assert_eq!(
                (
                    nested.self_status,
                    nested.cross_status,
                    nested.destroy_status
                ),
                (REENTRANT, REENTRANT, REENTRANT)
            );
            close(first);
            close(second);
            assert_eq!(unsafe { libc::pthread_mutex_destroy(&mut *mutex) }, 0);
        }
    }

    #[test]
    fn joined_worker_churn_and_reentry_on_all_backends() {
        let mut mutex = Box::new(unsafe { std::mem::zeroed::<libc::pthread_mutex_t>() });
        assert_eq!(
            unsafe { libc::pthread_mutex_init(&mut *mutex, ptr::null()) },
            0
        );
        for kind in 0..=8 {
            for _wave in 0..3 {
                let bridge = create(kind, (&mut *mutex as *mut libc::pthread_mutex_t).cast());
                let second = create(1, ptr::null_mut());
                let mut nested = Nested {
                    first: bridge,
                    second,
                    self_status: -1,
                    cross_status: -1,
                    destroy_status: -1,
                };
                assert_eq!(
                    unsafe {
                        dlock_bridge_execute(
                            bridge,
                            Some(nested_callback),
                            (&mut nested as *mut Nested).cast(),
                            ptr::null_mut(),
                        )
                    },
                    OK
                );
                assert_eq!(
                    (
                        nested.self_status,
                        nested.cross_status,
                        nested.destroy_status
                    ),
                    (REENTRANT, REENTRANT, REENTRANT)
                );
                let start = Arc::new(Barrier::new(9));
                let remote = Arc::new(AtomicU64::new(0));
                let workers: Vec<_> = (0..8)
                    .map(|id| {
                        let start = start.clone();
                        let remote = remote.clone();
                        let handle = bridge as usize;
                        thread::spawn(move || {
                            start.wait();
                            for ordinal in 0..32 {
                                let input = id * 32 + ordinal;
                                let mut data = Answer {
                                    input,
                                    output: 0,
                                    requester: thread::current().id(),
                                    remote: remote.clone(),
                                };
                                assert_eq!(
                                    unsafe {
                                        dlock_bridge_execute(
                                            handle as *mut DlockBridge,
                                            Some(answer_callback),
                                            (&mut data as *mut Answer).cast(),
                                            ptr::null_mut(),
                                        )
                                    },
                                    OK
                                );
                                assert_eq!(data.output, input ^ 0x61931abc);
                            }
                        })
                    })
                    .collect();
                start.wait();
                for worker in workers {
                    worker.join().unwrap();
                }
                // Join every caller, including USCL TLS teardown, before destroy.
                let mut global = DlockGlobalMetrics::default();
                assert_eq!(
                    unsafe { dlock_bridge_get_global_metrics(bridge, &mut global) },
                    OK
                );
                assert_eq!(global.enabled, u32::from(cfg!(feature = "profile")));
                #[cfg(feature = "profile")]
                {
                    assert_eq!((global.accepted_calls, global.completed_calls), (257, 257));
                    assert_eq!(global.rejected_reentrant, 1);
                    assert_eq!(global.active_calls, 0);
                    assert!((1..=257).contains(&global.peak_inflight_calls));
                }
                #[cfg(not(feature = "profile"))]
                assert_eq!(
                    (
                        global.accepted_calls,
                        global.completed_calls,
                        global.active_calls,
                        global.peak_inflight_calls,
                        global.rejected_reentrant
                    ),
                    (0, 0, 0, 0, 0)
                );
                assert_eq!(unsafe { dlock_bridge_destroy(bridge) }, OK);
                assert_eq!(unsafe { dlock_bridge_destroy(second) }, OK);
            }
        }
        assert_eq!(unsafe { libc::pthread_mutex_destroy(&mut *mutex) }, 0);
    }

    struct SerializedCounter {
        inside: AtomicBool,
        overlap: AtomicBool,
        value: AtomicU64,
    }

    struct CountRequest {
        counter: Arc<SerializedCounter>,
        input: u64,
        output: u64,
        application_status: i32,
    }

    unsafe extern "C" fn count_callback(raw: *mut c_void) {
        let request = unsafe { &mut *raw.cast::<CountRequest>() };
        let counter = &request.counter;
        if counter.inside.swap(true, AtomicOrdering::SeqCst) {
            counter.overlap.store(true, AtomicOrdering::Relaxed);
        }
        // Atomic storage keeps the test itself defined if exclusion is broken.
        // Deliberate load/store, not fetch_add: lost updates remain observable.
        let previous = counter.value.load(AtomicOrdering::Relaxed);
        thread::yield_now();
        request.application_status = if request.input % 7 == 0 { -73 } else { 0 };
        if request.application_status == 0 {
            counter
                .value
                .store(previous + request.input, AtomicOrdering::Relaxed);
        }
        request.output = request.input ^ 0x61931abc;
        counter.inside.store(false, AtomicOrdering::SeqCst);
    }

    #[test]
    fn multiple_handles_exclude_callbacks_publish_errors_and_survive_joined_churn() {
        for kind in 0..=8 {
            let mut mutexes: Vec<_> = (0..4)
                .map(|_| Box::new(unsafe { std::mem::zeroed::<libc::pthread_mutex_t>() }))
                .collect();
            let handles: Vec<usize> = mutexes
                .iter_mut()
                .map(|mutex| {
                    assert_eq!(
                        unsafe { libc::pthread_mutex_init(&mut **mutex, ptr::null()) },
                        0
                    );
                    create(kind, (&mut **mutex as *mut libc::pthread_mutex_t).cast()) as usize
                })
                .collect();
            let counters: Vec<_> = (0..handles.len())
                .map(|_| {
                    Arc::new(SerializedCounter {
                        inside: AtomicBool::new(false),
                        overlap: AtomicBool::new(false),
                        value: AtomicU64::new(0),
                    })
                })
                .collect();
            // Keep all handles alive across exited/replaced worker cohorts.
            // Thus each worker visits multiple instances, and TLS slots can
            // be recycled without freeing a node still owned by another lock.
            for _wave in 0..3 {
                let start = Arc::new(Barrier::new(9));
                let workers: Vec<_> = (0..8)
                    .map(|id| {
                        let start = start.clone();
                        let handles = handles.clone();
                        let counters = counters.clone();
                        thread::spawn(move || {
                            start.wait();
                            for ordinal in 0..32 {
                                for offset in 0..handles.len() {
                                    let index = (id + ordinal + offset) % handles.len();
                                    let input = (id * 32 + ordinal + index + 1) as u64;
                                    let mut request = CountRequest {
                                        counter: counters[index].clone(),
                                        input,
                                        output: 0,
                                        application_status: 1,
                                    };
                                    assert_eq!(
                                        unsafe {
                                            dlock_bridge_execute(
                                                handles[index] as *mut DlockBridge,
                                                Some(count_callback),
                                                (&mut request as *mut CountRequest).cast(),
                                                ptr::null_mut(),
                                            )
                                        },
                                        OK
                                    );
                                    assert_eq!(request.output, input ^ 0x61931abc);
                                    assert_eq!(
                                        request.application_status,
                                        if input % 7 == 0 { -73 } else { 0 }
                                    );
                                }
                            }
                            for handle in handles {
                                let mut metrics = DlockThreadMetrics::default();
                                assert_eq!(
                                    unsafe {
                                        dlock_bridge_get_thread_metrics(
                                            handle as *mut DlockBridge,
                                            &mut metrics,
                                        )
                                    },
                                    OK
                                );
                                assert_eq!(
                                    metrics.has_combiner_pass_ticks,
                                    u32::from(
                                        cfg!(feature = "profile") && (kind == 1 || kind == 2)
                                    )
                                );
                                if kind >= 3 {
                                    assert_eq!(
                                        metrics.executed_callbacks,
                                        if cfg!(feature = "profile") { 32 } else { 0 }
                                    );
                                }
                            }
                        })
                    })
                    .collect();
                start.wait();
                for worker in workers {
                    worker.join().unwrap();
                }
            }
            for (index, (handle, counter)) in handles.into_iter().zip(counters).enumerate() {
                assert!(
                    !counter.overlap.load(AtomicOrdering::Relaxed),
                    "overlapping callbacks: backend {kind}, handle {index}"
                );
                let expected: u64 = (1..=256)
                    .map(|input| input + index as u64)
                    .filter(|input| input % 7 != 0)
                    .sum::<u64>()
                    * 3;
                assert_eq!(counter.value.load(AtomicOrdering::Relaxed), expected);
                assert!(!counter.inside.load(AtomicOrdering::Relaxed));
                close(handle as *mut DlockBridge);
            }
            for mut mutex in mutexes {
                assert_eq!(unsafe { libc::pthread_mutex_destroy(&mut *mutex) }, 0);
            }
        }
    }

    struct Block {
        inside: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
        result: u64,
    }
    unsafe extern "C" fn blocked_callback(raw: *mut c_void) {
        let context = unsafe { &mut *raw.cast::<Block>() };
        context.inside.store(true, AtomicOrdering::Release);
        while !context.release.load(AtomicOrdering::Acquire) {
            thread::yield_now();
        }
        context.result = 909;
    }

    #[test]
    fn paused_callback_finishes_before_caller_join_and_destroy() {
        for kind in 0..=8 {
            let mut mutex = Box::new(unsafe { std::mem::zeroed::<libc::pthread_mutex_t>() });
            assert_eq!(
                unsafe { libc::pthread_mutex_init(&mut *mutex, ptr::null()) },
                0
            );
            let bridge = create(kind, (&mut *mutex as *mut libc::pthread_mutex_t).cast());
            let inside = Arc::new(AtomicBool::new(false));
            let release = Arc::new(AtomicBool::new(false));
            let entered = inside.clone();
            let free = release.clone();
            let handle = bridge as usize;
            let worker = thread::spawn(move || {
                let mut input = Block {
                    inside: entered,
                    release: free,
                    result: 0,
                };
                let status = unsafe {
                    dlock_bridge_execute(
                        handle as *mut DlockBridge,
                        Some(blocked_callback),
                        (&mut input as *mut Block).cast(),
                        ptr::null_mut(),
                    )
                };
                (status, input.result)
            });
            while !inside.load(AtomicOrdering::Acquire) {
                thread::yield_now();
            }
            release.store(true, AtomicOrdering::Release);
            assert_eq!(worker.join().unwrap(), (OK, 909));
            close(bridge);
            assert_eq!(unsafe { libc::pthread_mutex_destroy(&mut *mutex) }, 0);
        }
    }

    #[test]
    fn multiple_handles_progress_independently_of_a_paused_callback() {
        use std::sync::mpsc;
        use std::time::Duration;

        for kind in 0..=8 {
            let mut mutexes: Vec<_> = (0..2)
                .map(|_| Box::new(unsafe { std::mem::zeroed::<libc::pthread_mutex_t>() }))
                .collect();
            let handles: Vec<_> = mutexes
                .iter_mut()
                .map(|mutex| {
                    assert_eq!(
                        unsafe { libc::pthread_mutex_init(&mut **mutex, ptr::null()) },
                        0
                    );
                    create(kind, (&mut **mutex as *mut libc::pthread_mutex_t).cast()) as usize
                })
                .collect();
            let inside = Arc::new(AtomicBool::new(false));
            let release = Arc::new(AtomicBool::new(false));
            let first = handles[0];
            let entered = inside.clone();
            let free = release.clone();
            let paused = thread::spawn(move || {
                let mut request = Block {
                    inside: entered,
                    release: free,
                    result: 0,
                };
                let status = unsafe {
                    dlock_bridge_execute(
                        first as *mut DlockBridge,
                        Some(blocked_callback),
                        (&mut request as *mut Block).cast(),
                        ptr::null_mut(),
                    )
                };
                (status, request.result)
            });
            while !inside.load(AtomicOrdering::Acquire) {
                thread::yield_now();
            }
            let second = handles[1];
            let (sender, receiver) = mpsc::channel();
            let independent = thread::spawn(move || {
                let mut request = Answer {
                    input: 47,
                    output: 0,
                    requester: thread::current().id(),
                    remote: Arc::new(AtomicU64::new(0)),
                };
                let status = unsafe {
                    dlock_bridge_execute(
                        second as *mut DlockBridge,
                        Some(answer_callback),
                        (&mut request as *mut Answer).cast(),
                        ptr::null_mut(),
                    )
                };
                sender.send((status, request.output)).unwrap();
            });
            let independent_result = receiver.recv_timeout(Duration::from_secs(10));
            // Release and join even if the independence assertion will fail.
            release.store(true, AtomicOrdering::Release);
            assert_eq!(paused.join().unwrap(), (OK, 909));
            independent.join().unwrap();
            for handle in handles {
                close(handle as *mut DlockBridge);
            }
            for mut mutex in mutexes {
                assert_eq!(unsafe { libc::pthread_mutex_destroy(&mut *mutex) }, 0);
            }
            assert_eq!(independent_result.unwrap(), (OK, 47 ^ 0x61931abc));
        }
    }

    #[test]
    fn contenders_publish_caller_owned_results_across_workers() {
        for kind in [1, 2] {
            let bridge = create(kind, ptr::null_mut());
            let remote = Arc::new(AtomicU64::new(0));
            let barrier = Arc::new(Barrier::new(13));
            let workers: Vec<_> = (0..12)
                .map(|id| {
                    let barrier = barrier.clone();
                    let remote = remote.clone();
                    let handle = bridge as usize;
                    thread::spawn(move || {
                        barrier.wait();
                        for ordinal in 0..100 {
                            let input = id * 100 + ordinal;
                            let mut data = Answer {
                                input,
                                output: 0,
                                requester: thread::current().id(),
                                remote: remote.clone(),
                            };
                            assert_eq!(
                                unsafe {
                                    dlock_bridge_execute(
                                        handle as *mut DlockBridge,
                                        Some(answer_callback),
                                        (&mut data as *mut Answer).cast(),
                                        ptr::null_mut(),
                                    )
                                },
                                OK
                            );
                            assert_eq!(data.output, input ^ 0x61931abc);
                        }
                        #[cfg(feature = "profile")]
                        {
                            let mut stats = DlockThreadMetrics::default();
                            assert_eq!(
                                unsafe {
                                    dlock_bridge_get_thread_metrics(
                                        handle as *mut DlockBridge,
                                        &mut stats,
                                    )
                                },
                                OK
                            );
                            return stats.executed_callbacks;
                        }
                        #[cfg(not(feature = "profile"))]
                        return 0u64;
                    })
                })
                .collect();
            barrier.wait();
            let mut executed = 0;
            for worker in workers {
                executed += worker.join().unwrap();
            }
            #[cfg(feature = "profile")]
            assert_eq!(
                executed, 1200,
                "physical-worker execution counts must include remote callbacks"
            );
            #[cfg(not(feature = "profile"))]
            let _ = executed;
            assert!(
                remote.load(AtomicOrdering::Relaxed) > 0,
                "expected real remote combining with contending workers"
            );
            close(bridge);
        }
    }

    #[test]
    fn conventional_baselines_run_on_requester_and_survive_worker_churn() {
        for kind in 3..=8 {
            // Recreated handles exercise per-lock TLS and CFL's process-global
            // vLHT; each handle is freed only after its workers have joined.
            for _ in 0..3 {
                let bridge = create(kind, ptr::null_mut()) as usize;
                let remote = Arc::new(AtomicU64::new(0));
                let start = Arc::new(Barrier::new(9));
                let workers: Vec<_> = (0..8)
                    .map(|id| {
                        let start = start.clone();
                        let remote = remote.clone();
                        thread::spawn(move || {
                            start.wait();
                            for ordinal in 0..64 {
                                let input = id * 64 + ordinal;
                                let mut data = Answer {
                                    input,
                                    output: 0,
                                    requester: thread::current().id(),
                                    remote: remote.clone(),
                                };
                                let mut request = DlockRequestMetrics::default();
                                assert_eq!(
                                    unsafe {
                                        dlock_bridge_execute(
                                            bridge as *mut DlockBridge,
                                            Some(answer_callback),
                                            (&mut data as *mut Answer).cast(),
                                            &mut request,
                                        )
                                    },
                                    OK
                                );
                                assert_eq!(data.output, input ^ 0x61931abc);
                                assert_eq!(request.enabled, u32::from(cfg!(feature = "profile")));
                            }
                            let mut metrics = DlockThreadMetrics::default();
                            assert_eq!(
                                unsafe {
                                    dlock_bridge_get_thread_metrics(
                                        bridge as *mut DlockBridge,
                                        &mut metrics,
                                    )
                                },
                                OK
                            );
                            assert_eq!(metrics.has_combiner_pass_ticks, 0);
                            #[cfg(feature = "profile")]
                            assert_eq!(metrics.executed_callbacks, 64);
                        })
                    })
                    .collect();
                start.wait();
                for worker in workers {
                    worker.join().unwrap();
                }
                assert_eq!(remote.load(AtomicOrdering::Relaxed), 0);
                let handle = bridge as *mut DlockBridge;
                close(handle);
            }
        }
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn baseline_failstop_on_delegate_panic() {
        use std::os::unix::process::ExitStatusExt;
        if let Ok(kind) = std::env::var("DLOCK_BASELINE_PANIC_KIND") {
            let kind = kind.parse::<u32>().unwrap();
            let handle = create(kind, ptr::null_mut());
            unsafe { dlock_bridge_test_panic(handle) };
            panic!("panic hook unexpectedly returned");
        }
        for kind in 3..=8 {
            let outcome = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("tests::baseline_failstop_on_delegate_panic")
                .env("DLOCK_BASELINE_PANIC_KIND", kind.to_string())
                .output()
                .unwrap();
            assert_eq!(outcome.status.signal(), Some(libc::SIGABRT));
        }
    }

    #[cfg(feature = "profile")]
    #[test]
    fn profile_is_per_handle_and_attributes_service_to_executor() {
        let first = create(1, ptr::null_mut());
        let second = create(2, ptr::null_mut());
        let mut initial = DlockThreadMetrics::default();
        assert_eq!(
            unsafe { dlock_bridge_get_thread_metrics(first, &mut initial) },
            OK
        );
        assert_eq!(initial.enabled, 1);
        assert_eq!(initial.has_combiner_pass_ticks, 1);
        assert_eq!(initial.executed_callbacks, 0);
        assert_eq!(initial.combiner_pass_tsc_ticks, 0);
        let remote = Arc::new(AtomicU64::new(0));
        let mut data = Answer {
            input: 22,
            output: 0,
            requester: thread::current().id(),
            remote,
        };
        let mut request = DlockRequestMetrics::default();
        assert_eq!(
            unsafe {
                dlock_bridge_execute(
                    first,
                    Some(answer_callback),
                    (&mut data as *mut Answer).cast(),
                    &mut request,
                )
            },
            OK
        );
        assert_eq!(request.enabled, 1);
        assert!(request.service_ns > 0);
        assert!(request.service_tsc_ticks > 0);
        let mut own = DlockThreadMetrics::default();
        let mut other = DlockThreadMetrics::default();
        assert_eq!(
            unsafe { dlock_bridge_get_thread_metrics(first, &mut own) },
            OK
        );
        assert_eq!(
            unsafe { dlock_bridge_get_thread_metrics(second, &mut other) },
            OK
        );
        assert_eq!(own.executed_callbacks, 1);
        assert_eq!(own.executed_service_tsc_ticks, request.service_tsc_ticks);
        assert_eq!(own.executed_service_ns, request.service_ns);
        assert!(own.combiner_pass_tsc_ticks >= request.service_tsc_ticks);
        assert_eq!(other.executed_callbacks, 0);
        assert_eq!(other.combiner_pass_tsc_ticks, 0);
        let mut global = DlockGlobalMetrics::default();
        assert_eq!(
            unsafe { dlock_bridge_get_global_metrics(first, &mut global) },
            OK
        );
        assert_eq!(
            (
                global.accepted_calls,
                global.completed_calls,
                global.peak_inflight_calls,
                global.active_calls
            ),
            (1, 1, 1, 0)
        );
        close(first);
        close(second);
    }

    #[cfg(feature = "profile")]
    #[test]
    fn recycled_registration_does_not_reuse_prior_worker_metrics() {
        for kind in [1, 2] {
            let bridge = create(kind, ptr::null_mut()) as usize;
            for _ in 0..3 {
                let worker = thread::spawn(move || {
                    let handle = bridge as *mut DlockBridge;
                    let mut before = DlockThreadMetrics::default();
                    assert_eq!(
                        unsafe { dlock_bridge_get_thread_metrics(handle, &mut before) },
                        OK
                    );
                    assert_eq!(before.executed_callbacks, 0);
                    assert_eq!(before.combiner_pass_tsc_ticks, 0);
                    let mut data = Answer {
                        input: 3,
                        output: 0,
                        requester: thread::current().id(),
                        remote: Arc::new(AtomicU64::new(0)),
                    };
                    assert_eq!(
                        unsafe {
                            dlock_bridge_execute(
                                handle,
                                Some(answer_callback),
                                (&mut data as *mut Answer).cast(),
                                ptr::null_mut(),
                            )
                        },
                        OK
                    );
                    let mut after = DlockThreadMetrics::default();
                    assert_eq!(
                        unsafe { dlock_bridge_get_thread_metrics(handle, &mut after) },
                        OK
                    );
                    assert_eq!(after.executed_callbacks, 1);
                    assert!(after.combiner_pass_tsc_ticks > 0);
                });
                worker.join().unwrap();
            }
            close(bridge as *mut DlockBridge);
        }
    }
}
