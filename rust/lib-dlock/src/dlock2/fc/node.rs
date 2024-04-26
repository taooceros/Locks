use std::{
    cell::{SyncUnsafeCell, UnsafeCell},
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicPtr},
};

use crossbeam::utils::CachePadded;

use crate::dlock2::combiner_stat::CombinerSample;

pub struct Node<T> {
    pub age: UnsafeCell<u32>,
    pub active: CachePadded<AtomicBool>,
    pub data: SyncUnsafeCell<T>,
    pub complete: AtomicBool,
    pub next: AtomicPtr<Node<T>>,
    #[cfg(feature = "combiner_stat")]
    pub combiner_stat: CombinerSample,
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
            combiner_stat: CombinerSample::default().into(),
        }
    }
}
