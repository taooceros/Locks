use itertools::Itertools;
use nix::libc::FILENAME_MAX;
use strum::IntoEnumIterator;

use crate::benchmark::dlock2::proportional_counter::proportional_counter;
use crate::experiment::{DLock2Experiment, DLock2Option};
use crate::lock_target::DLock2Target;

use super::bencher::Bencher;

mod proportional_counter;

pub fn benchmark_dlock2(bencher: &Bencher, option: &DLock2Option) {
    let experiment = &option.experiment;

    let experiments = match experiment {
        Some(ref e) => vec![e],
        None => DLock2Experiment::to_vec_ref(),
    };

    for experiment in experiments {
        match experiment {
            DLock2Experiment::CounterProportional {
                cs_loops,
                non_cs_loops,
                file_name,
            } => {
                proportional_counter(
                    bencher,
                    file_name
                        .as_ref()
                        .unwrap_or(&format!(
                            "counter cs {:?} noncs {:?}",
                            cs_loops, non_cs_loops
                        ))
                        .as_str(),
                    option
                        .lock_targets
                        .as_ref()
                        .unwrap_or(&DLock2Target::iter().collect_vec())
                        .iter(),
                    cs_loops.iter().copied(),
                    non_cs_loops.iter().copied(),
                );
            }
        }
    }
}
