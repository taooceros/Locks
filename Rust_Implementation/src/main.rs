#![feature(sync_unsafe_cell)]
#![feature(fn_traits)]
#![feature(associated_type_bounds)]
#![feature(ptr_internals)]

use std::thread::available_parallelism;

mod benchmark;
pub mod ccsynch;
pub mod dlock;
pub mod flatcombining;
pub mod guard;
mod mutex_extension;
pub(crate) mod operation;
pub mod rcl;
pub(crate) mod syncptr;
#[cfg(test)]
mod unit_test;

fn main() {
    let num_cpus = available_parallelism().unwrap();
    benchmark::benchmark(num_cpus, num_cpus);
}
