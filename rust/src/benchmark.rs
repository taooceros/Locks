use serde_with::DurationNanoSeconds;
use strum::IntoEnumIterator;

use std::num::{NonZeroI64, NonZeroU64};
use std::path::Path;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::command_parser::experiment::Experiment;
use crate::command_parser::lock_target::LockTarget;
use crate::command_parser::*;

use self::bencher::Bencher;

mod bencher;
mod helper;
mod non_cs_counter;
mod one_three_ratio_counter;
mod proposion_counter;
mod response_time_single_addition;
mod response_time_variable;
mod subversion_job;

pub fn benchmark(
    num_cpu: usize,
    num_thread: usize,
    experiment: Option<Experiment>,
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

#[derive(Default)]
pub struct Records {
    pub id: usize,
    pub cpu_id: usize,
    pub thread_num: usize,
    pub cpu_num: usize,
    pub loop_count: u64,
    pub num_acquire: u64,
    pub job_length: Duration,
    pub is_combiner: Option<Vec<bool>>,
    pub response_times: Option<Vec<Duration>>,
    pub hold_time: Duration,
    pub combine_time: Option<NonZeroI64>,
    pub locktype: String,
    pub waiter_type: String,
}

impl Records {
    fn generate_plain_record(&self) -> Record {
        Record {
            id: self.id,
            cpu_id: self.cpu_id,
            thread_num: self.thread_num,
            cpu_num: self.cpu_num,
            job_length: self.job_length,
            is_combiner: None,
            loop_count: self.loop_count,
            num_acquire: self.num_acquire,
            response_time: None,
            hold_time: self.hold_time,
            #[cfg(feature = "combiner_stat")]
            combine_time: self.combine_time,
            locktype: self.locktype.clone(),
            waiter_type: self.waiter_type.clone(),
        }
    }

    pub fn to_records(&self) -> Box<dyn Iterator<Item = Record> + '_> {
        match (&self.response_times, &self.is_combiner) {
            (Some(response_times), Some(is_combiner)) => {
                Box::new(response_times.iter().zip(is_combiner.iter()).map(
                    |(response_time, is_combiner)| Record {
                        response_time: Some(*response_time),
                        is_combiner: Some(*is_combiner),
                        ..self.generate_plain_record()
                    },
                ))
            }
            (None, None) => Box::new(std::iter::once(self.generate_plain_record())),
            (None, Some(_)) => panic!("is_combiner is Some but response_times is None"),
            (Some(response_times), None) => {
                Box::new(response_times.iter().map(|response_time| Record {
                    response_time: Some(*response_time),
                    ..self.generate_plain_record()
                }))
            }
        }
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Record {
    pub id: usize,
    pub cpu_id: usize,
    pub thread_num: usize,
    pub cpu_num: usize,
    pub loop_count: u64,
    pub num_acquire: u64,
    #[serde_as(as = "DurationNanoSeconds")]
    pub job_length: Duration,
    pub is_combiner: Option<bool>,
    #[serde_as(as = "Option<DurationNanoSeconds>")]
    pub response_time: Option<Duration>,
    #[serde_as(as = "DurationNanoSeconds")]
    pub hold_time: Duration,
    #[cfg(feature = "combiner_stat")]
    pub combine_time: Option<NonZeroI64>,
    pub locktype: String,
    pub waiter_type: String,
}
