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
    /// Counter benchmark with array data (each CS iteration touches a distinct u64)
    CounterArray {
        #[arg(long = "cs", default_values_t = [100u64], value_delimiter = ',')]
        cs_loops: Vec<u64>,
        #[arg(long = "non-cs", default_values_t = [0u64], value_delimiter = ',')]
        non_cs_loops: Vec<u64>,
        #[arg(long = "file-name")]
        file_name: Option<String>,
        #[arg(long = "include-lock-free", default_value_t = false)]
        include_lock_free: bool,
        #[arg(long = "stat-hold-time", default_value_t = true)]
        stat_hold_time: bool,
        /// Number of u64 elements in the protected array (default: 4096 = 32 KiB).
        /// Use larger values to exceed L1 cache (>6144 = 48 KiB on Sapphire Rapids).
        #[arg(long = "array-size", default_value_t = 4096)]
        array_size: usize,
        /// Use random access pattern instead of sequential.
        /// Defeats hardware prefetching, making cache misses more expensive.
        #[arg(long = "random-access", default_value_t = false)]
        random_access: bool,
    },
    /// Concurrent HashMap benchmark with heterogeneous operations (get/put/scan)
    HashMap {
        /// Number of scanner threads (remaining threads are lookup)
        #[arg(long = "scan-threads", default_value_t = 2)]
        scan_threads: usize,
        /// Scan sizes (number of entries iterated per scan operation)
        #[arg(long = "scan-size", default_values_t = [100usize], value_delimiter = ',')]
        scan_sizes: Vec<usize>,
        /// Number of entries to pre-populate
        #[arg(long = "num-entries", default_value_t = 10000)]
        num_entries: usize,
        /// Get ratio among lookup operations (1.0 = all gets, 0.0 = all puts)
        #[arg(long = "get-ratio", default_value_t = 0.9)]
        get_ratio: f64,
        /// Zipfian skew parameter (0 = uniform, 0.99 = highly skewed)
        #[arg(long = "zipf-theta", default_value_t = 0.99)]
        zipf_theta: f64,
        /// Custom file name for output
        #[arg(long = "file-name")]
        file_name: Option<String>,
        /// Track per-CS hold time for fairness metrics
        #[arg(long = "stat-hold-time", default_value_t = true)]
        stat_hold_time: bool,
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
