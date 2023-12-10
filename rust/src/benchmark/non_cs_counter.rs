use std::time::Duration;

use super::bencher::LockBenchInfo;
use super::proposion_counter;

pub fn counter_one_three_non_cs_one() -> Box<dyn Fn(LockBenchInfo<u64>)> {
    proposion_counter::counter_proportional(
        vec![Duration::from_micros(10), Duration::from_micros(30)],
        vec![Duration::from_micros(10)],
        "counter_one_three_non_cs_one",
    )
}
