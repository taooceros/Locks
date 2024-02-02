use std::{num::ParseIntError, sync::OnceLock, time::Duration};

use clap::{Args, Subcommand};
use strum::{Display, EnumIter, IntoEnumIterator};

use crate::lock_target::{DLock1Target, WaiterType};

#[derive(Args, Debug, Clone, Default)]
pub struct DLock1Option {
    #[command(subcommand)]
    pub experiment: Option<DLock1Experiment>,
    #[arg(long, short, value_delimiter = ',', default_values_t = DLock1Target::iter())]
    pub targets: Vec<DLock1Target>,
    #[arg(global = true, long, short, default_value = "all")]
    pub waiter: WaiterType,
}

#[derive(Debug, Clone, Display, Subcommand, EnumIter)]
pub enum Experiment {
    DLock2 {
        #[command(subcommand)]
        subcommand: Option<DLock2Experiment>,
    },
    DLock1(DLock1Option),
}

#[derive(Debug, Clone, Display, Subcommand, EnumIter)]
pub enum DLock1Experiment {
    CounterRatioOneThree,
    CounterSubversion,
    CounterRatioOneThreeNonCS,
    CounterProportional {
        #[arg(value_parser = parse_duration, long = "cs", default_value = "100000", value_delimiter = ',')]
        cs_durations: Vec<Duration>,
        #[arg(value_parser = parse_duration, long = "non-cs", default_value = "0", value_delimiter = ',')]
        non_cs_durations: Vec<Duration>,
        #[arg(long = "file-name", default_value = "proportional_counter")]
        file_name: String,
    },
    ResponseTimeSingleAddition,
    ResponseTimeRatioOneThree,
}

impl DLock1Experiment {
    pub fn to_vec_ref() -> Vec<&'static Self> {
        static INSTANCE: OnceLock<Vec<DLock1Experiment>> = OnceLock::new();

        INSTANCE
            .get_or_init(|| DLock1Experiment::iter().collect())
            .iter()
            .collect()
    }
}

#[derive(Debug, Clone, Display, Subcommand, EnumIter)]
pub enum DLock2Experiment {
    CounterRatioOneThree,
}

impl DLock2Experiment {
    pub fn to_vec_ref() -> Vec<&'static Self> {
        static INSTANCE: OnceLock<Vec<DLock2Experiment>> = OnceLock::new();

        INSTANCE
            .get_or_init(|| DLock2Experiment::iter().collect())
            .iter()
            .collect()
    }
}

fn parse_duration(arg: &str) -> Result<Duration, ParseIntError> {
    let nanos = arg.parse::<u64>()?;
    Ok(Duration::from_nanos(nanos))
}
