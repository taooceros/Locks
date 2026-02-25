//! Per-thread queue node for ShflLock (AQS).
//!
//! Each node is cache-line padded (128 bytes) to eliminate false sharing.

use std::{
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU16, AtomicU8},
};

/// Lock status values matching the C AQS_STATUS_* constants.
pub const STATUS_WAIT: u8 = 0;
pub const STATUS_LOCKED: u8 = 1;

/// Per-thread queue node, matching `aqs_node_t` in the C implementation.
#[derive(Debug)]
#[repr(align(128))]
pub struct QNode {
    pub next: AtomicPtr<QNode>,
    /// Lock status: `STATUS_WAIT` while waiting, `STATUS_LOCKED` when granted.
    pub lstatus: AtomicU8,
    /// Shuffle leader flag: set to 1 when this node should shuffle.
    pub sleader: AtomicBool,
    /// Waiter/shuffle count for batch limiting.
    pub wcount: AtomicU16,
    /// NUMA node ID of the owning thread.
    pub nid: i32,
    /// Pointer to last node visited during a shuffle pass.
    pub last_visited: AtomicPtr<QNode>,
}

impl QNode {
    pub const fn new() -> Self {
        QNode {
            next: AtomicPtr::new(null_mut()),
            lstatus: AtomicU8::new(STATUS_WAIT),
            sleader: AtomicBool::new(false),
            wcount: AtomicU16::new(0),
            nid: 0,
            last_visited: AtomicPtr::new(null_mut()),
        }
    }
}

// SAFETY: All fields are atomic; cross-thread access is properly ordered.
unsafe impl Send for QNode {}
unsafe impl Sync for QNode {}
