use std::{
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicPtr},
};

pub struct Node<T> {
    pub age: SyncUnsafeCell<u32>,
    pub data: SyncUnsafeCell<T>,
    pub completed: AtomicBool,
    pub wait: AtomicBool,
    pub next: AtomicPtr<Node<T>>,
    #[cfg(feature = "combiner_stat")]
    pub combiner_time_stat: u64,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Node {
            age: SyncUnsafeCell::new(0),
            data: SyncUnsafeCell::new(unsafe { MaybeUninit::uninit().assume_init() }),
            completed: AtomicBool::new(false),
            wait: AtomicBool::new(false),
            next: AtomicPtr::new(std::ptr::null_mut()),
            #[cfg(feature = "combiner_stat")]
            combiner_time_stat: 0,
        }
    }
}
