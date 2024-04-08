use std::{borrow::Borrow, cell::{RefCell, RefMut}, collections::HashMap, fs::File, path::Path, time::Duration};

use arrow::{datatypes::Schema, record_batch::RecordBatch};
use arrow_ipc::writer::{FileWriter, IpcWriteOptions};
use serde::{Deserialize, Serialize};
use serde_arrow::schema::{SchemaLike, SerdeArrowSchema, TracingOptions};

use crate::benchmark::helper::create_plain_writer;

use super::bencher::Bencher;

#[derive(Default, Serialize, Deserialize)]
pub struct Records {
    pub id: usize,
    pub cpu_id: usize,
    pub thread_num: usize,
    pub cpu_num: usize,
    pub loop_count: u64,
    pub num_acquire: u64,
    pub cs_length: u64,
    pub duration: u64,
    pub non_cs_length: Option<u64>,
    pub combiner_latency: Vec<u64>,
    pub waiter_latency: Vec<u64>,
    pub hold_time: u64,
    pub combine_time: Option<u64>,
    pub locktype: String,
    pub waiter_type: String,
}

impl Records {
    pub fn from_bencher(bencher: &Bencher) -> Self {
        Self {
            cpu_num: bencher.num_cpu,
            thread_num: bencher.num_thread,
            duration: bencher.duration,
            ..Default::default()
        }
    }
}

pub fn write_results<'a>(output_path: &Path, file_name: &str, results: impl Borrow<Vec<Records>>) {
    thread_local! {
        static WRITERS: RefCell<HashMap<String, FileWriter<std::fs::File>>> = HashMap::new().into();
    }

    let fields = SerdeArrowSchema::from_type::<Records>(TracingOptions::default())
        .unwrap()
        .to_arrow_fields()
        .unwrap();
    let arrays = serde_arrow::to_arrow(&fields, results.borrow()).unwrap();

    let schema = Schema::new(fields);

    WRITERS.with(move |cell| {
        let mut map : RefMut<HashMap<String, FileWriter<File>>> = cell.borrow_mut();
        
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

        let batch = RecordBatch::try_new(schema.into(), arrays).unwrap();

        writer.unwrap().write(&batch).expect("Failed to write");
    });
}
