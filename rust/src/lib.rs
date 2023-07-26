#![feature(sync_unsafe_cell)]
#![feature(pointer_is_aligned)]
#![feature(type_alias_impl_trait)]
#![feature(atomic_from_ptr)]
#![feature(thread_id_value)]

pub mod ccsynch;
pub mod ccsynch_fair_ban;
pub mod dlock;
pub mod flatcombining;
pub mod fc_fair_ban;
pub mod fc_fair_ban_slice;
pub mod fc_fair_skiplist;
pub mod guard;
pub mod rcl;

mod mutex_extension;
mod syncptr;
#[cfg(test)]
mod unit_test;
pub mod spin_lock;

pub unsafe trait RawSimpleLock {
    fn new() -> Self;

    /// Non-blocking: Try locking. If succeeding, return true, or false.
    fn try_lock(&self) -> bool;

    /// Blocking: Get locking or wait until getting locking
    fn lock(&self);

    /// Release lock
    fn unlock(&self);
}