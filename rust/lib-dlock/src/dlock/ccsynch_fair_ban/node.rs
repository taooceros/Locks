use std::sync::atomic::*;

use crossbeam::utils::CachePadded;

use crate::{dlock::DLockDelegate, parker::Parker};

#[derive(Default)]
pub(crate) struct Node<T, P>
where
    P: Parker,
{
    pub(super) f: CachePadded<Option<*mut dyn DLockDelegate<T>>>,
    pub(super) wait: P,
    pub(super) completed: AtomicBool,
    pub(super) next: AtomicPtr<Node<T, P>>,
    pub(super) current_cs: u64,
    #[cfg(feature = "combiner_stat")]
    pub(super) combiner_time_stat: u64,
}

unsafe impl<T, P: Parker> Send for Node<T, P> {}
unsafe impl<T, P: Parker> Sync for Node<T, P> {}

impl<T, P: Parker> Node<T, P> {
    pub fn new() -> Node<T, P> {
        Node {
            f: CachePadded::default(),
            wait: Default::default(),
            completed: AtomicBool::default(),
            next: AtomicPtr::default(),
            current_cs: 0,
            combiner_time_stat: 0,
        }
    }
}
