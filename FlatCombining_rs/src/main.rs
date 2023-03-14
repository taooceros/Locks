#![feature(sync_unsafe_cell)]
#![feature(fn_traits)]
#![feature(associated_type_bounds)]

use std::sync::{Arc, Mutex};
use std::thread;

use flatcombining::FcLock;

use crate::flatcombining::FcGuard;

pub mod flatcombining;
pub mod benchmark;

// I have some magic semantics for some synchronization primitive!
#[derive(Debug, Clone, Copy)]
pub struct I32Unsafe(*mut i32);

unsafe impl Send for I32Unsafe {}
unsafe impl Sync for I32Unsafe {}

fn main() {
    benchmark::benchmark();
}
