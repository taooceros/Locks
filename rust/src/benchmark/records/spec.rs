use std::{
    collections::HashMap,
    iter::{once, repeat, Once},
};

use clap::builder;
use derive_builder::Builder;
use libdlock::dlock2::CombinerSample;
use serde::{Deserialize, Serialize};

use crate::benchmark::bencher::Bencher;

#[derive(Debug, Default, Serialize, Deserialize, Clone, Builder)]
pub struct Spec {
    pub id: usize,
    pub cpu_id: usize,
    pub thread_num: usize,
    pub cpu_num: usize,
    pub loop_count: u64,
    pub num_acquire: u64,
    pub duration: u64,
    pub target_name: String,
    #[builder(default)]
    pub cs_length: u64,
    #[builder(default)]
    pub non_cs_length: u64,
    #[builder(default)]
    pub hold_time: u64,
    #[builder(default)]
    pub waiter_type: Option<String>,
}

impl SpecBuilder {
    pub fn with_bencher(&mut self, bencher: &Bencher) -> &mut Self {
        self.thread_num = bencher.num_thread.into();
        self.cpu_num = bencher.num_cpu.into();
        self.duration = bencher.duration.into();
        self
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Latency {
    pub combiner_latency: Vec<u64>,
    pub waiter_latency: Vec<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CombinerStatistics {
    pub combine_time: Vec<u64>,
    pub combine_size: Vec<usize>,
}

impl CombinerStatistics {
    pub fn from_combiner_sample(sample: &CombinerSample) -> Self {
        Self {
            combine_size: sample
                .combine_size
                .iter()
                .flat_map(|(k, v)| repeat(*k).take(*v))
                .collect(),
            combine_time: sample.combine_time.clone(),
        }
    }
}
