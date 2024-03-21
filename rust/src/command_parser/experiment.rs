use std::{num::ParseIntError, sync::OnceLock, time::Duration};

use clap::{Args, Subcommand, ValueEnum};
use strum::{Display, EnumIter, IntoEnumIterator};

use crate::{
    benchmark::dlock2::queue::extension::LockFreeQueue,
    lock_target::{DLock1Target, DLock2Target, WaiterType},
};

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
        #[arg(long = "cs", default_values_t = [1000u64], value_delimiter = ',')]
        cs_loops: Vec<u64>,
        #[arg(long = "non-cs", default_values_t = [0u64], value_delimiter = ',')]
        non_cs_loops: Vec<u64>,
        #[arg(long = "file-name")]
        file_name: Option<String>,
        #[arg(long = "inlcude-lock-free", default_value_t = false)]
        include_lock_free: bool,
        #[arg(long = "stat-hold-time", default_value_t = true)]
        stat_hold_time: bool,
    },
    FetchAndMultiply {
        #[arg(long = "inlcude-lock-free", default_value_t = true)]
        include_lock_free: bool,
    },
    Queue {
        #[arg(long = "sequencial-queue-type", default_value = "linked-list")]
        seq_queue_type: SeqQueueType,
        #[arg(long = "lock-free-queues")]
        lock_free_queues: Vec<LockFreeQueue>,
    },
    PriorityQueue {
        #[arg(long = "sequencial-pq-type", default_value = "binary-heap")]
        sequencial_pq_type: SeqPQType,
    },
}

#[derive(Default, Debug, Clone, ValueEnum)]
pub enum SeqQueueType {
    #[default]
    LinkedList,
    VecDeque,
}

#[derive(Debug, Default, Clone, Display, EnumIter, ValueEnum)]
pub enum SeqPQType {
    BTreeSet,
    #[default]
    BinaryHeap,
    PairingHeap,
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
