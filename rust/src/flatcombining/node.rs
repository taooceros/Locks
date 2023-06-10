use std::cell::SyncUnsafeCell;

use linux_futex::*;

use crate::{guard::DLockGuard, syncptr::SyncMutPtr};

pub(super) struct NodeData<T> {
    pub(super) age: i32,
    pub(super) active: bool,
    pub(super) f: Option<*mut (dyn FnMut(&mut DLockGuard<T>))>,
    pub(super) waiter: Futex<Private>, // id: i32,
}

unsafe impl<T> Sync for NodeData<T> {}

unsafe impl<T> Send for NodeData<T> {}

pub(super) struct Node<T> {
    pub(super) value: SyncUnsafeCell<NodeData<T>>,
    pub(super) next: Option<SyncMutPtr<Node<T>>>,
}
