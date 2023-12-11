#![feature(sync_unsafe_cell)]
#![feature(pointer_is_aligned)]
#![feature(type_alias_impl_trait)]
#![feature(atomic_from_ptr)]
#![feature(thread_id_value)]
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

pub mod ccsynch;
pub mod ccsynch_fair_ban;
pub mod dlock;
pub mod fc;
pub mod fc_fair_ban;
pub mod fc_fair_ban_slice;
pub mod fc_sl_naive;
pub mod fc_sl;
pub mod guard;
pub mod parker;
pub mod rcl;

mod mutex_extension;
pub mod spin_lock;
mod syncptr;
pub mod u_scl;
#[cfg(test)]
mod unit_test;

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

pub unsafe trait RawSimpleLock {
    fn new() -> Self;

    /// Non-blocking: Try locking. If succeeding, return true, or false.
    fn try_lock(&self) -> bool;

    /// Blocking: Get locking or wait until getting locking
    fn lock(&self);

    /// Release lock
    fn unlock(&self);
}
