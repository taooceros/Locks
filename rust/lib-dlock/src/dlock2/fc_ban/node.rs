use std::{
    cell::{SyncUnsafeCell, UnsafeCell},
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicPtr},
};

pub struct Node<T> {
    pub age: UnsafeCell<u32>,
    pub active: AtomicBool,
    pub data: SyncUnsafeCell<MaybeUninit<T>>,
    pub complete: AtomicBool,
    pub next: AtomicPtr<Node<T>>,
    pub banned_until: SyncUnsafeCell<u64>,
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
            active: AtomicBool::new(false),
            complete: AtomicBool::new(false),
            data: SyncUnsafeCell::new(MaybeUninit::uninit()),
            next: AtomicPtr::default(),
            banned_until: 0.into(),
            #[cfg(feature = "combiner_stat")]
            combiner_time_stat: 0,
        }
    }
}
