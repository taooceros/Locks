#![feature(sync_unsafe_cell)]
#![feature(fn_traits)]
#![feature(associated_type_bounds)]
#![feature(ptr_internals)]


mod benchmark;
#[cfg(test)]
mod unit_test;
pub mod flatcombining;
pub mod ccsynch;
pub mod rcl;
pub mod guard;
pub mod dlock;
mod mutex_extension;
pub(crate) mod operation;
pub(crate) mod syncptr;


fn main() {
    // benchmark::benchmark();
    
}
