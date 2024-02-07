use crate::benchmark::dlock2::fetch_and_multiply::fetch_and_multiply;
use itertools::Itertools;

use strum::IntoEnumIterator;

use crate::benchmark::dlock2::proportional_counter::proportional_counter;
use crate::experiment::{DLock2Experiment, DLock2Option};
use crate::lock_target::DLock2Target;

use super::bencher::Bencher;

mod fetch_and_multiply;
mod proportional_counter;
mod queue;

pub fn benchmark_dlock2(bencher: &Bencher, option: &DLock2Option) {
    let experiment = &option.experiment;

    let experiments = match experiment {
        Some(ref e) => vec![e],
        None => DLock2Experiment::to_vec_ref(),
    };

    for experiment in experiments {
        let mut default_targets = None;

        let targets = option
            .lock_targets
            .as_ref()
            .unwrap_or_else(|| default_targets.insert(DLock2Target::iter().collect_vec()));

        let mut name_maybe = None;

        match experiment {
            DLock2Experiment::CounterProportional {
                cs_loops,
                non_cs_loops,
                file_name,
                include_lock_free,
            } => proportional_counter(
                bencher,
                file_name.as_deref().unwrap_or_else(|| {
                    name_maybe.insert(format!(
                        "counter cs {:?} noncs {:?}",
                        cs_loops, non_cs_loops
                    ))
                }),
                targets.iter(),
                cs_loops.iter().copied(),
                non_cs_loops.iter().copied(),
                *include_lock_free,
            ),
            DLock2Experiment::FetchAndMultiply { include_lock_free } => {
                fetch_and_multiply(bencher, targets.iter(), *include_lock_free)
            }
        }
    }
}
