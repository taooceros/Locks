use std::{
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicPtr},
};

#[derive(Debug)]
pub struct Node<T> {
    pub data: SyncUnsafeCell<MaybeUninit<T>>,
    pub completed: AtomicBool,
    pub wait: AtomicBool,
    pub next: AtomicPtr<Node<T>>,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Node {
            data: MaybeUninit::uninit().into(),
            completed: AtomicBool::new(false),
            wait: AtomicBool::new(false),
            next: AtomicPtr::new(std::ptr::null_mut()),
        }
    }
}
