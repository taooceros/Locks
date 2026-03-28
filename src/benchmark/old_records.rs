use arrow::{array::*, datatypes::*};
use serde_with::DurationNanoSeconds;
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_with::serde_as;

#[derive(Default)]
pub struct Records {
    pub id: usize,
    pub cpu_id: usize,
    pub thread_num: usize,
    pub cpu_num: usize,
    pub loop_count: u64,
    pub num_acquire: u64,
    pub cs_length: Duration,
    pub non_cs_length: Option<Duration>,
    pub is_combiner: Option<Vec<Option<bool>>>,
    pub response_times: Option<Vec<Option<Duration>>>,
    pub hold_time: Duration,
    pub combine_time: Option<u64>,
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
            cs_length: self.cs_length,
            non_cs_length: self.non_cs_length,
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
                        response_time: *response_time,
                        is_combiner: Some(*is_combiner),
                        ..self.generate_plain_record()
                    },
                ))
            }
            (None, None) => Box::new(std::iter::once(self.generate_plain_record())),
            (None, Some(_)) => panic!("is_combiner is Some but response_times is None"),
            (Some(response_times), None) => {
                Box::new(response_times.iter().map(|response_time| Record {
                    response_time: *response_time,
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
    pub cs_length: Duration,
    #[serde_as(as = "Option<DurationNanoSeconds>")]
    pub non_cs_length: Option<Duration>,
    pub is_combiner: Option<Option<bool>>,
    #[serde_as(as = "Option<DurationNanoSeconds>")]
    pub response_time: Option<Duration>,
    #[serde_as(as = "DurationNanoSeconds")]
    pub hold_time: Duration,
    #[cfg(feature = "combiner_stat")]
    pub combine_time: Option<u64>,
    pub locktype: String,
    pub waiter_type: String,
}

#[derive(Debug, Default)]
pub struct RecordsBuilder {
    id: UInt64Builder,
    cpu_id: UInt64Builder,
    thread_num: UInt64Builder,
    cpu_num: UInt64Builder,
    loop_count: UInt64Builder,
    num_acquire: UInt64Builder,
    cs_length: UInt64Builder,
    is_combiner: ListBuilder<BooleanBuilder>,
    response_time: ListBuilder<DurationNanosecondBuilder>,
    hold_time: UInt64Builder,
    combine_time: UInt64Builder,
    locktype: StringBuilder,
    waiter_type: StringBuilder,
}

impl RecordsBuilder {
    pub fn append(&mut self, row: &Records) {
        self.id.append_value(row.id as u64);
        self.cpu_id.append_value(row.cpu_id as u64);
        self.thread_num.append_value(row.thread_num as u64);
        self.cpu_num.append_value(row.cpu_num as u64);
        self.loop_count.append_value(row.loop_count);
        self.num_acquire.append_value(row.num_acquire);
        self.cs_length.append_value(row.cs_length.as_nanos() as u64);
        self.is_combiner
            .append_option(row.is_combiner.as_ref().map(|v| v.iter().copied()));
        self.response_time.append_option(
            row.response_times
                .as_ref()
                .map(|v| v.iter().map(|v| v.as_ref().map(|d| d.as_nanos() as i64))),
        );
        self.hold_time.append_value(row.hold_time.as_nanos() as u64);
        self.combine_time.append_option(row.combine_time);
        self.locktype.append_value(row.locktype.clone());
        self.waiter_type.append_value(row.waiter_type.clone());
    }

    fn get_field() -> Fields {
        let id_field = Arc::new(Field::new("id", arrow::datatypes::DataType::UInt64, false));

        let cpu_id_field = Arc::new(Field::new(
            "cpu_id",
            arrow::datatypes::DataType::UInt64,
            false,
        ));

        let thread_num_field = Arc::new(Field::new(
            "thread_num",
            arrow::datatypes::DataType::UInt64,
            false,
        ));

        let cpu_num_field = Arc::new(Field::new(
            "cpu_num",
            arrow::datatypes::DataType::UInt64,
            false,
        ));

        let loop_count_field = Arc::new(Field::new(
            "loop_count",
            arrow::datatypes::DataType::UInt64,
            false,
        ));

        let num_acquire_field = Arc::new(Field::new(
            "num_acquire",
            arrow::datatypes::DataType::UInt64,
            false,
        ));

        let job_length_field = Arc::new(Field::new(
            "job_length",
            arrow::datatypes::DataType::UInt64,
            false,
        ));

        let is_combiner_inner_field = Arc::new(Field::new(
            "item",
            arrow::datatypes::DataType::Boolean,
            true,
        ));
        let is_combiner_field = Arc::new(Field::new(
            "is_combiner",
            DataType::List(is_combiner_inner_field),
            true,
        ));

        let response_time_inner_field = Arc::new(Field::new(
            "item",
            arrow::datatypes::DataType::Duration(arrow::datatypes::TimeUnit::Nanosecond),
            true,
        ));
        let response_time_field = Arc::new(Field::new(
            "response_time",
            DataType::List(response_time_inner_field),
            true,
        ));

        let hold_time_field = Arc::new(Field::new(
            "hold_time",
            arrow::datatypes::DataType::UInt64,
            false,
        ));

        let combine_time_field = Arc::new(Field::new(
            "combine_time",
            arrow::datatypes::DataType::UInt64,
            true,
        ));
        let locktype_field = Arc::new(Field::new(
            "locktype",
            arrow::datatypes::DataType::Utf8,
            false,
        ));

        let waiter_type_field = Arc::new(Field::new(
            "waiter_type",
            arrow::datatypes::DataType::Utf8,
            false,
        ));

        vec![
            id_field,
            cpu_id_field,
            thread_num_field,
            cpu_num_field,
            loop_count_field,
            num_acquire_field,
            job_length_field,
            is_combiner_field,
            response_time_field,
            hold_time_field,
            combine_time_field,
            locktype_field,
            waiter_type_field,
        ]
        .into()
    }

    pub fn get_schema() -> &'static Schema {
        static SCHEMA: OnceLock<Schema> = OnceLock::new();

        SCHEMA.get_or_init(|| Schema::new(Self::get_field()))
    }

    pub fn finish(&mut self) -> StructArray {
        let id = Arc::new(self.id.finish()) as ArrayRef;

        let cpu_id = Arc::new(self.cpu_id.finish()) as ArrayRef;

        let thread_num = Arc::new(self.thread_num.finish()) as ArrayRef;

        let cpu_num = Arc::new(self.cpu_num.finish()) as ArrayRef;

        let loop_count = Arc::new(self.loop_count.finish()) as ArrayRef;

        let num_acquire = Arc::new(self.num_acquire.finish()) as ArrayRef;

        let job_length = Arc::new(self.cs_length.finish()) as ArrayRef;

        let is_combiner = Arc::new(self.is_combiner.finish()) as ArrayRef;

        let response_time = Arc::new(self.response_time.finish()) as ArrayRef;

        let hold_time = Arc::new(self.hold_time.finish()) as ArrayRef;

        let combine_time = Arc::new(self.combine_time.finish()) as ArrayRef;

        let locktype = Arc::new(self.locktype.finish()) as ArrayRef;

        let waiter_type = Arc::new(self.waiter_type.finish()) as ArrayRef;

        StructArray::new(
            Self::get_field(),
            vec![
                id,
                cpu_id,
                thread_num,
                cpu_num,
                loop_count,
                num_acquire,
                job_length,
                is_combiner,
                response_time,
                hold_time,
                combine_time,
                locktype,
                waiter_type,
            ],
            None,
        )
    }
}

impl<'a> Extend<&'a Records> for RecordsBuilder {
    fn extend<T: IntoIterator<Item = &'a Records>>(&mut self, iter: T) {
        iter.into_iter().for_each(|row| self.append(row));
    }
}
