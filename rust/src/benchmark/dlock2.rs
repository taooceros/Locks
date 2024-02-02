use crate::experiment::DLock2Experiment;

use super::bencher::Bencher;

mod proportional_counter;


pub fn benchmark_dlock2(bencher: &Bencher, experiment: &Option<DLock2Experiment>) {
    let experiments = match experiment {
        Some(ref e) => vec![e],
        None => DLock2Experiment::to_vec_ref(),
    };

    for experiment in experiments {
        match experiment {
            DLock2Experiment::CounterRatioOneThree => {
                // counter_one_three_benchmark(bencher);
            }
        }
    }
}
