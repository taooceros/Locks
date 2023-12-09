use arrow::array::{
    ArrayRef, BooleanBuilder, DurationNanosecondBuilder, DurationSecondBufferBuilder, Int32Builder,
    Int64Builder, ListBuilder, StringArray, StringBuilder, StructArray, UInt64Array, UInt64Builder,
};
use arrow::datatypes::{DataType, Field, Fields, Schema, SchemaBuilder};
use arrow::record_batch::RecordBatch;
use serde_with::DurationNanoSeconds;
use strum::IntoEnumIterator;

use std::cell::OnceCell;
use std::iter::{once, Once};
use std::num::{NonZeroI64, NonZeroU64};
use std::path::Path;

use std::sync::{Arc, OnceLock};
use std::time::Duration;
use std::vec;

use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::command_parser::experiment::Experiment;
use crate::command_parser::lock_target::LockTarget;
use crate::command_parser::*;

use self::bencher::Bencher;

mod bencher;
mod helper;
mod records;
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

