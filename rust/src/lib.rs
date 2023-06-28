#![feature(sync_unsafe_cell)]
#![feature(pointer_is_aligned)]
#![feature(type_alias_impl_trait)]
#![feature(atomic_from_ptr)]

pub mod ccsynch;
pub mod dlock;
pub mod flatcombining;
pub mod fc_fair_ban;
pub mod fc_fair_ban_slice;
pub mod guard;
pub mod rcl;

mod mutex_extension;
mod operation;
mod syncptr;
#[cfg(test)]
mod unit_test;
pub mod raw_spin_lock;

pub unsafe trait RawSimpleLock {
    fn new() -> Self;

    /// Non-blocking: Try locking. If succeeding, return true, or false.
    fn try_lock(&self) -> bool;

    /// Blocking: Get locking or wait until getting locking
    fn lock(&self);

    /// Release lock
    fn unlock(&self);
}