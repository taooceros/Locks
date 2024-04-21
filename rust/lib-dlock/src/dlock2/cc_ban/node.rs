use std::{
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicPtr},
};

pub struct Node<T> {
    pub data: SyncUnsafeCell<T>,
    pub completed: AtomicBool,
    pub wait: AtomicBool,
    pub panelty: SyncUnsafeCell<u64>,
    pub next: AtomicPtr<Node<T>>,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Node {
            data: SyncUnsafeCell::new(unsafe { MaybeUninit::uninit().assume_init() }),
            completed: AtomicBool::new(false),
            wait: AtomicBool::new(false),
            panelty: SyncUnsafeCell::new(0),
            next: AtomicPtr::default(),
        }
    }
}
