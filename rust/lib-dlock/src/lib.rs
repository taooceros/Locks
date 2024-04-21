#![feature(sync_unsafe_cell)]
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
pub mod sequential_priority_queue;
mod atomic_extension;


include!(concat!(env!("OUT_DIR"), "/bindings.rs"));