use std::time::Duration;

use crate::benchmark::bencher::LockBenchInfo;

use super::proposion_counter;

pub fn counter_one_three_benchmark() -> Box<dyn Fn(LockBenchInfo<u64>)> {
    let cs_durations = vec![Duration::from_micros(10), Duration::from_micros(30)];

    let non_cs_durations = vec![Duration::ZERO];

    proposion_counter::counter_proportional(cs_durations, non_cs_durations, "counter_one_three")
}
