//! C AQS (ShflLock) binding implementing [`lock_api::RawMutex`].
//!
//! Wraps the C AQS implementation from `c/shfllock/aqs.c` behind
//! [`RawMutex`] so it can be used with [`super::spinlock::DLock2Wrapper`].
//! Per-thread `aqs_node_t` nodes are stored in a [`ThreadLocal`].

use std::cell::SyncUnsafeCell;
use std::mem::MaybeUninit;

use lock_api::{GuardSend, RawMutex};
use thread_local::ThreadLocal;

use crate::{aqs_lock, aqs_mutex_t, aqs_node_t, aqs_trylock, aqs_unlock};

/// Newtype around `aqs_node_t` to implement `Send` + `Sync`.
///
/// SAFETY: The AQS lock protocol ensures that a node is only accessed by
/// its owning thread (while filling in the request) and by the lock holder
/// (while traversing the queue under the lock).  The raw pointer fields
/// inside `aqs_node_t` do not introduce data races.
#[repr(transparent)]
struct AqsNode(SyncUnsafeCell<aqs_node_t>);

impl AqsNode {
    fn new() -> Self {
        AqsNode(SyncUnsafeCell::new(unsafe {
            MaybeUninit::<aqs_node_t>::zeroed().assume_init()
        }))
    }

    fn get(&self) -> *mut aqs_node_t {
        self.0.get()
    }
}

// SAFETY: See comment on AqsNode.
unsafe impl Send for AqsNode {}
unsafe impl Sync for AqsNode {}

/// Newtype around `aqs_mutex_t` to implement `Send` + `Sync`.
#[repr(transparent)]
struct AqsMutex(SyncUnsafeCell<aqs_mutex_t>);

// SAFETY: The C AQS lock serialises all concurrent accesses.
unsafe impl Send for AqsMutex {}
unsafe impl Sync for AqsMutex {}

/// Raw C AQS lock implementing [`lock_api::RawMutex`].
pub struct RawCAqs {
    lock: AqsMutex,
    local_node: ThreadLocal<AqsNode>,
}

impl std::fmt::Debug for RawCAqs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawCAqs").finish()
    }
}

// SAFETY: The C AQS lock serialises all concurrent accesses.
unsafe impl Send for RawCAqs {}
unsafe impl Sync for RawCAqs {}

unsafe impl RawMutex for RawCAqs {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = RawCAqs {
        lock: AqsMutex(SyncUnsafeCell::new(unsafe {
            MaybeUninit::zeroed().assume_init()
        })),
        local_node: ThreadLocal::new(),
    };

    type GuardMarker = GuardSend;

    fn lock(&self) {
        let node = self.local_node.get_or(AqsNode::new);
        unsafe {
            aqs_lock(self.lock.0.get(), node.get());
        }
    }

    fn try_lock(&self) -> bool {
        unsafe { aqs_trylock(self.lock.0.get()) == 0 }
    }

    unsafe fn unlock(&self) {
        aqs_unlock(self.lock.0.get());
    }
}
