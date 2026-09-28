use std::{
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicU64},
};

use crossbeam::utils::CachePadded;

pub struct Node<T> {
    /// Accumulated CS time. Written by the owner (newcomer initialisation in
    /// `push_node`) and by combiners; atomic because a combiner may still hold
    /// a stale queue entry for this node while the owner re-enrolls.
    pub usage: AtomicU64,
    pub active: CachePadded<AtomicBool>,
    pub data: SyncUnsafeCell<MaybeUninit<T>>,
    pub complete: AtomicBool,
    #[cfg(feature = "combiner_stat")]
    // Written/read only by this node's ThreadLocal owner; combiners hold &Node.
    pub combiner_time_stat: SyncUnsafeCell<u64>,
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
            data: SyncUnsafeCell::new(MaybeUninit::uninit()),
            #[cfg(feature = "combiner_stat")]
            combiner_time_stat: SyncUnsafeCell::new(0),
        }
    }
}
