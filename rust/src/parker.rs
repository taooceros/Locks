use std::time::Duration;
use std::fmt::Debug;
use serde::Serialize;

#[derive(PartialEq, Eq, Debug)]
pub enum State {
    Empty,
    Parked,
    Prenotified,
    Notified,
}

pub trait Parker: Debug + Default + Serialize {
    fn wait(&self);
    fn wait_timeout(&self, timeout: Duration) -> Result<(), ()>;
    fn wake(&self);
    fn state(&self) -> State;
    fn reset(&self);
    fn prewake(&self);
    fn name() -> &'static str;
}

pub mod block_parker;
pub mod spin_parker;
