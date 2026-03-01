//! Ticket lock — a simple, fair (FIFO) spin lock.
//!
//! Each thread atomically fetches-and-increments a `next_ticket` counter to
//! obtain a unique turn number, then spins on a `now_serving` counter until
//! its turn arrives.  Unlocking increments `now_serving` to hand off to the
//! next thread in FIFO order.
//!
//! Fair but not scalable: all threads spin on the same `now_serving` cache
//! line, causing O(N) invalidations per handoff.  Useful as a baseline for
//! comparing against queue locks (MCS, CLH) and delegation locks.

use std::sync::atomic::{AtomicU32, Ordering};

use lock_api::{GuardSend, RawMutex};

/// Raw ticket lock compatible with [`lock_api::RawMutex`].
///
/// Suitable for use with [`super::spinlock::DLock2Wrapper`].
#[derive(Debug)]
pub struct RawTicketLock {
    next_ticket: AtomicU32,
    now_serving: AtomicU32,
}

unsafe impl Send for RawTicketLock {}
unsafe impl Sync for RawTicketLock {}

unsafe impl RawMutex for RawTicketLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = RawTicketLock {
        next_ticket: AtomicU32::new(0),
        now_serving: AtomicU32::new(0),
    };

    type GuardMarker = GuardSend;

    fn lock(&self) {
        let ticket = self.next_ticket.fetch_add(1, Ordering::Relaxed);
        while self.now_serving.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
        }
    }

    fn try_lock(&self) -> bool {
        let current = self.now_serving.load(Ordering::Relaxed);
        self.next_ticket
            .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    unsafe fn unlock(&self) {
        // Increment now_serving to hand off to the next ticket holder.
        self.now_serving.fetch_add(1, Ordering::Release);
    }
}
