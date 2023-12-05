use clap::{Subcommand};
use strum::{Display, EnumIter};

#[derive(Debug, Clone, Copy, Display, Subcommand, EnumIter)]
pub enum Experiment {
    CounterRatioOneThree,
    CounterSubversion,
    CounterNonCS,
    CounterProportional,
    ResponseTimeSingleAddition,
    ResponseTimeRatioOneThree,
}
