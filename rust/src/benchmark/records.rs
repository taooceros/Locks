use std::{
    borrow::Borrow,
    cell::{RefCell, RefMut},
    collections::HashMap,
    fs::File,
    path::Path,
};

use arrow::{
    datatypes::{FieldRef, Schema},
    record_batch::RecordBatch,
};
use arrow_ipc::writer::{FileWriter, IpcWriteOptions};
use libdlock::dlock2::CombinerSample;
use serde::{Deserialize, Serialize};
use serde_arrow::schema::{SchemaLike, SerdeArrowSchema, TracingOptions};

use crate::benchmark::helper::create_plain_writer;

use super::bencher::Bencher;

pub mod spec;

#[derive(Default, Serialize, Deserialize)]
pub struct Records {
    #[serde(flatten)]
    pub spec: spec::Spec,
    #[serde(flatten)]
    pub latency: spec::Latency,
    #[serde(flatten)]
    pub combiner_stat: spec::CombinerStatistics,
}

pub fn write_results<'a>(output_path: &Path, file_name: &str, results: impl Borrow<Vec<Records>>) {
    thread_local! {
        static WRITERS: RefCell<HashMap<String, FileWriter<std::fs::File>>> = HashMap::new().into();
    }

    let fields = Vec::<FieldRef>::from_samples(
        results.borrow(),
        TracingOptions::default()
            .map_as_struct(true)
            .coerce_numbers(true)
            .allow_null_fields(true),
    )
    .unwrap();
    let batch = serde_arrow::to_record_batch(&fields, results.borrow()).unwrap();

    let schema = Schema::new(fields);

    WRITERS.with(move |cell| {
        let mut map: RefMut<HashMap<String, FileWriter<File>>> = cell.borrow_mut();

        let file_path = output_path.join(format!("{file_name}.arrow"));
        let file_path_str = file_path.to_str().unwrap();

        let writer = if map.contains_key(file_path_str) {
            map.get_mut(file_path_str)
        } else {
            let option =
                IpcWriteOptions::try_new(8, false, arrow::ipc::MetadataVersion::V5).unwrap();
            // .try_with_compression(Some(CompressionType::ZSTD))
            // .expect("Failed to create compression option");

            map.insert(
                file_path_str.to_owned(),
                FileWriter::try_new_with_options(
                    create_plain_writer(&file_path).expect("Failed to create writer"),
                    &schema,
                    option,
                )
                .expect("Failed to create file writer"),
            );

            map.get_mut(file_path_str)
        };

        writer.unwrap().write(&batch).expect("Failed to write");
    });
}
