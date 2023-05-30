#![feature(sync_unsafe_cell)]
#![feature(pointer_is_aligned)]
#![feature(type_alias_impl_trait)]

pub mod ccsynch;
pub mod dlock;
pub mod flatcombining;
pub mod guard;
pub mod rcl;

mod mutex_extension;
mod operation;
mod syncptr;
#[cfg(test)]
mod unit_test;
