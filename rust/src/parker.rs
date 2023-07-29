pub trait Parker: Default {
    fn wait(&self);
    fn wake(&self);
    fn reset(&self);
    fn prewake(&self);
}

pub mod block_parker;
pub mod spin_block_parker;
pub mod spin_parker;