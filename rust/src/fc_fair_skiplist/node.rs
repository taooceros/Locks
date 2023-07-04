use std::{sync::atomic::AtomicBool};

use crossbeam::utils::CachePadded;
use linux_futex::{Futex, Private};

use crate::dlock::DLockDelegate;

pub struct Node<T> {
    pub(super) age: u32,
    pub(super) active: AtomicBool,
    pub(super) usage: u64,
    pub(super) f: CachePadded<Option<*mut (dyn DLockDelegate<T>)>>,
    pub(super) waiter: Futex<Private>, // id: i32,
    pub(super) combiner_time: i64,
    #[cfg(feature = "combiner_stat")]
    pub(super) combiner_time_stat: i64,
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
            combiner_time: 0,
            combiner_time_stat: 0,
        }
    }
}
