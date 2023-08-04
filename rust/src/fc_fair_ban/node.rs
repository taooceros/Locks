use std::{ptr::null_mut, sync::atomic::AtomicBool};

use crossbeam::utils::CachePadded;
use linux_futex::{Futex, Private};

use crate::{dlock::DLockDelegate, parker::Parker};

pub(super) struct Node<T, P: Parker> {
    pub(super) age: u32,
    pub(super) active: AtomicBool,
    pub(super) usage: isize,
    pub(super) f: CachePadded<Option<*mut (dyn DLockDelegate<T>)>>,
    pub(super) next: *mut Node<T, P>,
    pub(super) parker: P, // id: i32,
    pub(super) banned_until: u64,
    #[cfg(feature = "combiner_stat")]
    pub(super) combiner_time_stat: i64,
}

unsafe impl<T, P: Parker> Send for Node<T, P> {}
unsafe impl<T, P: Parker> Sync for Node<T, P> {}

impl<T, P: Parker> Node<T, P> {
    pub(super) fn new() -> Self {
        Self {
            age: 0,
            active: AtomicBool::new(false),
            usage: 0,
            f: CachePadded::new(None),
            parker: Default::default(),
            next: null_mut(),
            banned_until: 0,
            #[cfg(feature = "combiner_stat")]
            combiner_time_stat: 0,
        }
    }
}
