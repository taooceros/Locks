use std::{ptr::null_mut, sync::atomic::AtomicBool};

use crossbeam::utils::CachePadded;

use crate::{dlock::DLockDelegate, parker::Parker};

pub(super) struct Node<T, P>
where
    P: Parker,
{
    pub(super) age: u32,
    pub(super) active: AtomicBool,
    pub(super) f: CachePadded<Option<*mut (dyn DLockDelegate<T>)>>,
    pub(super) next: *mut Node<T, P>,
    pub(super) parker: P, // id: i32,
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
            f: CachePadded::new(None),
            parker: Default::default(),
            next: null_mut(),
            #[cfg(feature = "combiner_stat")]
            combiner_time_stat: 0,
        }
    }
}
