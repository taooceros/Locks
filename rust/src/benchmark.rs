use strum::IntoEnumIterator;

use std::path::Path;

use crate::command_parser::experiment::Experiment;
use crate::command_parser::lock_target::LockTarget;
use crate::command_parser::*;

use self::bencher::Bencher;

mod bencher;
mod helper;
mod non_cs_counter;
mod one_three_ratio_counter;
mod proposion_counter;
mod records;
mod response_time_single_addition;
mod response_time_variable;
mod subversion_job;
mod dlock2;

pub fn benchmark(
    num_cpu: usize,
    num_thread: usize,
    experiment: Option<&Experiment>,
    options: &GlobalOpts,
) {
    let bencher = Bencher::new(
        num_cpu,
        num_thread,
        experiment,
        match &options.lock_target {
            Some(t) => t.clone(),
            None => LockTarget::iter().collect(),
        },
        Path::new(&options.output_path)
            .to_path_buf()
            .into_boxed_path(),
        options.waiter,
        options.stat_response_time,
        options.duration,
        options.verbose,
    );

    bencher.benchmark();
}
