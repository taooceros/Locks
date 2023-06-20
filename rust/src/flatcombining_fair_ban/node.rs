use std::{ptr::null_mut, sync::atomic::AtomicBool};

use crossbeam::utils::CachePadded;
use linux_futex::{Futex, Private};

use crate::dlock::DLockDelegate;

pub(super) struct Node<T> {
    pub(super) age: u32,
    pub(super) active: AtomicBool,
    pub(super) usage: isize,
    pub(super) f: CachePadded<Option<*mut (dyn DLockDelegate<T>)>>,
    pub(super) next: *mut Node<T>,
    pub(super) waiter: Futex<Private>, // id: i32,
    pub(super) banned_until: u64,
}

unsafe impl<T> Send for Node<T> {}
unsafe impl<T> Sync for Node<T> {}

impl<T> Node<T> {
    pub(super) fn new() -> Self {
        Self {
            age: 0,
            active: AtomicBool::new(false),
            usage: 0,
            f: CachePadded::new(None),
            waiter: Futex::new(0),
            next: null_mut(),
            banned_until : 0
        }
    }
}
