#![feature(sync_unsafe_cell)]
#![feature(fn_traits)]
#![feature(associated_type_bounds)]
#![feature(ptr_internals)]


mod benchmark;
mod unit_test;
pub mod flatcombining;
pub mod ccsynch;
pub mod rcl;


// I have some magic semantics for some synchronization primitive!
#[derive(Debug, Clone, Copy)]
pub struct I32Unsafe(*mut i32);

unsafe impl Send for I32Unsafe {}
unsafe impl Sync for I32Unsafe {}

fn main() {
    benchmark::benchmark();
    // unit_test::test_lock();
}
