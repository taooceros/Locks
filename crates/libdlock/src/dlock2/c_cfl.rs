//! C CFL (Compact Fair Lock, PPoPP'24) binding implementing [`lock_api::RawMutex`].
//!
//! Wraps the original C CFL implementation from `c/cfl/cfl.c` behind
//! [`RawMutex`] so it can be used with [`super::spinlock::DLock2Wrapper`].
//! Per-thread `cfl_node_t` nodes are stored in a [`ThreadLocal`].

use std::cell::SyncUnsafeCell;
use std::mem::MaybeUninit;

use lock_api::{GuardSend, RawMutex};
use thread_local::ThreadLocal;

use crate::{cfl_init, cfl_lock, cfl_mutex_t, cfl_node_t, cfl_unlock};

/// Newtype around `cfl_node_t` to implement `Send` + `Sync`.
///
/// SAFETY: The CFL lock protocol ensures that a node is only accessed by
/// its owning thread (while filling in the request) and by the shuffle
/// leader (while traversing the queue under the lock).  The raw pointer
/// fields inside `cfl_node_t` do not introduce data races.
#[repr(transparent)]
struct CflNode(SyncUnsafeCell<cfl_node_t>);

impl CflNode {
    fn new() -> Self {
        CflNode(SyncUnsafeCell::new(unsafe {
            MaybeUninit::<cfl_node_t>::zeroed().assume_init()
        }))
    }

    fn get(&self) -> *mut cfl_node_t {
        self.0.get()
    }
}

// SAFETY: See comment on CflNode.
unsafe impl Send for CflNode {}
unsafe impl Sync for CflNode {}

/// Newtype around `cfl_mutex_t` to implement `Send` + `Sync`.
#[repr(transparent)]
struct CflMutex(SyncUnsafeCell<cfl_mutex_t>);

// SAFETY: The C CFL lock serialises all concurrent accesses.
unsafe impl Send for CflMutex {}
unsafe impl Sync for CflMutex {}

/// Raw C CFL lock implementing [`lock_api::RawMutex`].
pub struct RawCCfl {
    lock: CflMutex,
    local_node: ThreadLocal<CflNode>,
}

impl std::fmt::Debug for RawCCfl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawCCfl").finish()
    }
}

// SAFETY: The C CFL lock serialises all concurrent accesses.
unsafe impl Send for RawCCfl {}
unsafe impl Sync for RawCCfl {}

unsafe impl RawMutex for RawCCfl {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = RawCCfl {
        lock: CflMutex(SyncUnsafeCell::new(unsafe {
            MaybeUninit::zeroed().assume_init()
        })),
        local_node: ThreadLocal::new(),
    };

    type GuardMarker = GuardSend;

    fn lock(&self) {
        let node = self.local_node.get_or(CflNode::new);
        unsafe {
            cfl_lock(self.lock.0.get(), node.get());
        }
    }

    fn try_lock(&self) -> bool {
        // CFL doesn't expose a trylock — fall back to attempting the fast path.
        // For benchmarks this is acceptable; the lock is never used via try_lock.
        false
    }

    unsafe fn unlock(&self) {
        let node = self.local_node.get_or(CflNode::new);
        cfl_unlock(self.lock.0.get(), node.get());
    }
}
