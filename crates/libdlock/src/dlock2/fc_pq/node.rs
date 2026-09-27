use std::{
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicU64},
};

use atomic_enum::atomic_enum;
use crossbeam::utils::CachePadded;

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
    pub data: SyncUnsafeCell<MaybeUninit<T>>,
    pub complete: AtomicBool,
    #[cfg(feature = "combiner_stat")]
    // Written/read only by this node's ThreadLocal owner; the queue holds &Node.
    pub combiner_time_stat: SyncUnsafeCell<u64>,
    #[cfg(feature = "fcpq_cached_tid")]
    // Owner's thread id, the ring entry's tie-breaker; cached once (plan D6).
    // A ThreadLocal slot recycled after its thread exits keeps the old id,
    // still unique per node, which is all the tie-breaker needs.
    pub tid: u64,
    #[cfg(feature = "fcpq_fast_path_stat")]
    // Fast-path hits; one writer (the owner, Relaxed load+store), any reader.
    pub fast_path_hits: AtomicU64,
}

impl<T> Node<T> {
    /// Called by `ThreadLocal::get_or` on the owning thread.
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
            #[cfg(feature = "fcpq_cached_tid")]
            tid: std::thread::current().id().as_u64().get(),
            #[cfg(feature = "fcpq_fast_path_stat")]
            fast_path_hits: AtomicU64::new(0),
        }
    }
}
