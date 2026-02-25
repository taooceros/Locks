#![feature(sync_unsafe_cell)]
#![feature(pointer_is_aligned)]
#![feature(type_alias_impl_trait)]
#![feature(thread_id_value)]
#![feature(trait_alias)]

#[cfg(not(target_arch = "x86_64"))]
compile_error!("This crate requires x86_64 (uses __rdtscp and x86-specific C code)");

#[cfg(test)]
mod dlock2_unit_test;
pub mod spin_lock;
mod syncptr;
pub mod u_scl;
#[cfg(test)]
mod unit_test;

mod atomic_extension;
pub mod c_binding;
pub mod dlock;
pub mod dlock2;
pub mod parker;
pub mod sequential_priority_queue;

#[allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]
mod bindings {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
pub use bindings::*;
