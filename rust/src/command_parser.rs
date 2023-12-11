use std::thread::available_parallelism;

use clap::*;

use strum::IntoEnumIterator;

use self::{
    experiment::Experiment,
    lock_target::{LockTarget, WaiterType},
};

pub mod experiment;
pub mod lock_target;

#[derive(Debug, Parser)]
#[clap(name = "lock counter benchmark", version)]
/// Benchmark Utility
pub struct App {
    #[command(subcommand)]
    pub lock_target: Option<Experiment>,
    #[command(flatten)]
    pub global_opts: GlobalOpts,
}

#[derive(Debug, Args)]
pub struct GlobalOpts {
    #[arg(global = true, num_args(0..), value_delimiter = ',', value_terminator("."), long, short, default_values_t = [available_parallelism().unwrap().get()].to_vec())]
    pub threads: Vec<usize>,
    #[arg(global = true, num_args(0..), value_delimiter = ',', value_terminator("."), long, short, default_values_t = [available_parallelism().unwrap().get()].to_vec())]
    pub cpus: Vec<usize>,
    #[arg(global = true, long, short, default_value = "../visualization/output")]
    pub output_path: String,
    #[arg(global = true, long, short, default_value = "all")]
    pub waiter: WaiterType,
    #[arg(global = true, long, short)]
    pub stat_response_time: bool,
    #[arg(global = true, long, short, default_value = "5")]
    pub duration: u64,
    #[arg(global = true, long, short, value_delimiter = ',')]
    pub lock_target: Option<Vec<LockTarget>>,
    #[arg(global = true, long, short)]
    pub verbose: bool,
}
