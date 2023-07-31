use std::time::Duration;

#[derive(PartialEq, Eq)]
pub enum State {
    Empty,
    Parked,
    Prenotified,
    Notified,
}

pub trait Parker: Default {
    fn wait(&self);
    fn wait_timeout(&self, timeout: Duration) -> Result<(), ()>;
    fn wake(&self);
    fn state(&self) -> State;
    fn reset(&self);
    fn prewake(&self);
}

pub mod block_parker;
pub mod spin_parker;
