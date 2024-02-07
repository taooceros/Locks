use std::{num::ParseIntError, sync::OnceLock, time::Duration};

use clap::{Args, Subcommand};
use strum::{Display, EnumIter, IntoEnumIterator};

use crate::{benchmark::dlock2::queue::LockFreeQueue, lock_target::{DLock1Target, DLock2Target, WaiterType}};

#[derive(Args, Debug, Clone, Default)]
pub struct DLock1Option {
    #[command(subcommand)]
    pub experiment: Option<DLock1Experiment>,
    #[arg(long, short, value_delimiter = ',')]
    pub lock_targets: Option<Vec<DLock1Target>>,
    #[arg(global = true, long, short, default_value = "all")]
    pub waiter: WaiterType,
}

#[derive(Args, Debug, Clone)]
pub struct DLock2Option {
    #[command(subcommand)]
    pub experiment: Option<DLock2Experiment>,
    #[arg(long, short, value_delimiter = ',')]
    pub lock_targets: Option<Vec<DLock2Target>>,
}

#[derive(Debug, Clone, Display, Subcommand)]
pub enum Experiment {
    DLock2(DLock2Option),
    DLock1(DLock1Option),
}

#[derive(Debug, Clone, Display, Subcommand, EnumIter)]
pub enum DLock1Experiment {
    CounterRatioOneThree,
    CounterSubversion,
    CounterRatioOneThreeNonCS,
    CounterProportional {
        #[arg(value_parser = parse_duration, long = "cs", default_values = ["1000"], value_delimiter = ',')]
        cs_durations: Vec<Duration>,
        #[arg(value_parser = parse_duration, long = "non-cs", default_values = ["0"], value_delimiter = ',')]
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
    CounterProportional {
        #[arg(long = "cs", default_values_t = [1000usize], value_delimiter = ',')]
        cs_loops: Vec<usize>,
        #[arg(long = "non-cs", default_values_t = [0usize], value_delimiter = ',')]
        non_cs_loops: Vec<usize>,
        #[arg(long = "file-name")]
        file_name: Option<String>,
        #[arg(long = "inlcude-lock-free", default_value_t = true)]
        include_lock_free: bool,
    },
    FetchAndMultiply {
        #[arg(long = "inlcude-lock-free", default_value_t = true)]
        include_lock_free: bool,
    },
    Queue {
        #[arg(long = "lock-free-queues")]
        lock_free_queues: Vec<LockFreeQueue>,
    }
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
