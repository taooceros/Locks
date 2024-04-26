use std::{
    cell::{SyncUnsafeCell},
    mem::MaybeUninit,
    sync::atomic::{AtomicBool},
};

use crossbeam::utils::CachePadded;

use crate::dlock2::combiner_stat::CombinerSample;

pub struct Node<T> {
    pub usage: u64,
    pub active: CachePadded<AtomicBool>,
    pub data: SyncUnsafeCell<T>,
    pub complete: AtomicBool,
    #[cfg(feature = "combiner_stat")]
    pub combiner_stat: CombinerSample,
}

impl<T> Node<T> {
    pub(crate) fn new() -> Node<T>
    where
        T: Send,
    {
        Node {
            usage: 0,
            active: AtomicBool::new(false).into(),
            complete: AtomicBool::new(false),
            data: unsafe { MaybeUninit::uninit().assume_init() },
            #[cfg(feature = "combiner_stat")]
            combiner_stat: CombinerSample::default(),
        }
    }
}

impl<T> Drop for Node<T> {
    fn drop(&mut self) {
        // don't drop data
    }
}
