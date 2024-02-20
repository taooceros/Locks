use std::{
    cell::{SyncUnsafeCell, UnsafeCell},
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicPtr},
};

use crossbeam::utils::CachePadded;

pub struct Node<T> {
    pub age: UnsafeCell<u32>,
    pub active: CachePadded<AtomicBool>,
    pub data: SyncUnsafeCell<T>,
    pub complete: AtomicBool,
    pub next: AtomicPtr<Node<T>>,
    #[cfg(feature = "combiner_stat")]
    pub combiner_time_stat: u64,
}

impl<T> Node<T> {
    pub(crate) fn new() -> Node<T>
    where
        T: Send,
    {
        Node {
            age: 0.into(),
            active: AtomicBool::new(false).into(),
            complete: AtomicBool::new(false),
            data: unsafe { MaybeUninit::uninit().assume_init() },
            next: AtomicPtr::default(),
            #[cfg(feature = "combiner_stat")]
            combiner_time_stat: 0,
        }
    }
}

impl<T> Drop for Node<T> {
    fn drop(&mut self) {
        // don't drop anything
    }
}
