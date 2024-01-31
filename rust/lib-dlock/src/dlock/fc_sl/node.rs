use std::sync::atomic::AtomicBool;

use crossbeam::utils::CachePadded;

use crate::{dlock::DLockDelegate, parker::Parker};

pub struct Node<T, P>
where
    P: Parker + 'static,
{
    pub(super) should_combine: AtomicBool,
    pub(super) finish: AtomicBool,
    pub(super) usage: u64,
    pub(super) f: CachePadded<Option<*mut (dyn DLockDelegate<T>)>>,
    pub(super) parker: P, // id: i32,
    #[cfg(feature = "combiner_stat")]
    pub(super) combiner_time_stat: i64,
}

unsafe impl<T, P: Parker> Send for Node<T, P> {}
unsafe impl<T, P: Parker> Sync for Node<T, P> {}

impl<T, P: Parker> Node<T, P> {
    pub(super) fn new() -> Self {
        Self {
            should_combine: AtomicBool::new(false),
            finish: AtomicBool::new(false),
            usage: 0,
            f: CachePadded::new(None),
            parker: Default::default(),
            combiner_time_stat: 0,
        }
    }
}
