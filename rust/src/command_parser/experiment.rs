use clap::ValueEnum;
use strum::{Display, EnumIter};

#[derive(Debug, Clone, Copy, Display, ValueEnum, EnumIter)]
pub enum Experiment {
    CounterRatioOneThree,
    CounterSubversion,
    CounterNonCS,
    ResponseTimeSingleAddition,
    ResponseTimeRatioOneThree,
}
