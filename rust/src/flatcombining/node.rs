use std::cell::SyncUnsafeCell;

use crossbeam::utils::CachePadded;
use linux_futex::*;

use crate::dlock::DLockDelegate;
use crate::syncptr::SyncMutPtr;

pub(super) struct NodeData<T> {
    pub(super) age: i32,
    pub(super) active: bool,
    pub(super) f: CachePadded<Option<*mut (dyn DLockDelegate<T>)>>,
    pub(super) waiter: Futex<Private>, // id: i32,
    #[cfg(feature = "combiner_stat")]
    pub(super) combiner_time_stat: i64,
}

unsafe impl<T> Sync for NodeData<T> {}

unsafe impl<T> Send for NodeData<T> {}

pub(super) struct Node<T> {
    pub(super) value: SyncUnsafeCell<NodeData<T>>,
    pub(super) next: Option<SyncMutPtr<Node<T>>>,
}
