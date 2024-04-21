use std::{
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicU64},
};

use atomic_enum::atomic_enum;
use crossbeam::utils::CachePadded;

use crate::dlock2::CombinerStatistics;

#[atomic_enum]
#[derive(PartialEq)]
pub enum ActiveState {
    Inactive,
    Attempted,
    Active,
}

#[derive(Debug)]
pub struct Node<T> {
    pub usage: AtomicU64,
    pub active: CachePadded<AtomicBool>,
    pub data: SyncUnsafeCell<T>,
    pub complete: AtomicBool,
    #[cfg(feature = "combiner_stat")]
    pub combiner_stat: CombinerStatistics,
}

impl<T> Node<T> {
    pub(crate) fn new() -> Node<T>
    where
        T: Send,
    {
        Node {
            usage: AtomicU64::new(0),
            active: AtomicBool::new(false).into(),
            complete: AtomicBool::new(false),
            data: unsafe { MaybeUninit::uninit().assume_init() },
            #[cfg(feature = "combiner_stat")]
            combiner_stat: CombinerStatistics::default(),
        }
    }
}