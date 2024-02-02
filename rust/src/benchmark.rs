use strum::IntoEnumIterator;

use std::path::Path;

use crate::command_parser::experiment::Experiment;
use crate::command_parser::lock_target::LockTarget;
use crate::command_parser::*;

use self::bencher::Bencher;

mod bencher;
mod dlock;
mod dlock2;
mod helper;
mod records;

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
