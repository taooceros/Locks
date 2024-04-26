use std::{
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicPtr},
};

pub struct Node<T> {
    pub data: SyncUnsafeCell<T>,
    pub completed: AtomicBool,
    pub wait: AtomicBool,
    pub next: AtomicPtr<Node<T>>,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Node {
            data: SyncUnsafeCell::new(unsafe { MaybeUninit::uninit().assume_init() }),
            completed: AtomicBool::new(false),
            wait: AtomicBool::new(false),
            next: AtomicPtr::new(std::ptr::null_mut()),
        }
    }
}
