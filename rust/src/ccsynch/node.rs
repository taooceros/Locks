use std::sync::atomic::*;

use crossbeam::utils::CachePadded;

use crate::{dlock::DLockDelegate, parker::Parker};

#[derive(Default)]
pub(crate) struct Node<T, P : Parker> {
    pub(super) f: CachePadded<Option<*mut dyn DLockDelegate<T>>>,
    pub(super) wait: P,
    pub(super) completed: AtomicBool,
    pub(super) next: AtomicPtr<Node<T, P>>,
    #[cfg(feature = "combiner_stat")]
    pub(super) combiner_time_stat: i64,
}

unsafe impl<T, W: Parker> Send for Node<T, W> {}
unsafe impl<T, W: Parker> Sync for Node<T, W> {}

impl<T, W: Parker> Node<T, W> {
    pub fn new() -> Node<T, W> {
        Node {
            f: CachePadded::default(),
            wait: Default::default(),
            completed: Default::default(),
            next: AtomicPtr::default(),
            combiner_time_stat: 0,
        }
    }
}
