use std::{time::Duration};

pub trait Parker: Default {
    fn wait(&self);
    fn wait_timeout(&self, timeout: Duration);
    fn wake(&self);
    fn reset(&self);
    fn prewake(&self);
}

pub mod block_parker;
pub mod spin_parker;
