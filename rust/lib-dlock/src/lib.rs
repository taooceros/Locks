#![feature(sync_unsafe_cell)]
#![feature(pointer_is_aligned)]
#![feature(type_alias_impl_trait)]
#![feature(thread_id_value)]
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![feature(trait_alias)]

pub mod spin_lock;
mod syncptr;
pub mod u_scl;
#[cfg(test)]
mod unit_test;

pub mod dlock;
pub mod dlock2;
pub mod parker;
pub mod c_binding;
mod atomic_extension;

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
