use std::sync::atomic::*;

use crossbeam::utils::CachePadded;

use crate::dlock::DLockDelegate;

#[derive(Default)]
pub(crate) struct Node<T> {
    pub(super) f: CachePadded<Option<*mut dyn DLockDelegate<T>>>,
    pub(super) wait: AtomicBool,
    pub(super) completed: AtomicBool,
    pub(super) next: AtomicPtr<Node<T>>,
    #[cfg(feature = "combiner_stat")]
    pub(super) combiner_time_stat: i64,
}

unsafe impl<T> Send for Node<T> {}
unsafe impl<T> Sync for Node<T> {}

impl<T> Node<T> {
    pub fn new() -> Node<T> {
        Node {
            f: CachePadded::default(),
            wait: AtomicBool::default(),
            completed: AtomicBool::default(),
            next: AtomicPtr::default(),
            combiner_time_stat: 0,
        }
    }
}
