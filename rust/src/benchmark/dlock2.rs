use std::collections::{BTreeSet, BinaryHeap, LinkedList, VecDeque};

use crate::benchmark::dlock2::fetch_and_multiply::fetch_and_multiply;
use crate::experiment::*;
use itertools::Itertools;

use strum::IntoEnumIterator;

use crate::benchmark::dlock2::proportional_counter::proportional_counter;
use crate::experiment::{DLock2Experiment, DLock2Option};
use crate::lock_target::DLock2Target;

use super::bencher::Bencher;

mod fetch_and_multiply;
pub mod priority_queue;
mod proportional_counter;
pub mod queue;

pub fn benchmark_dlock2(bencher: &Bencher, option: &DLock2Option) {
    let experiment = &option.experiment;

    let experiments = match experiment {
        Some(ref e) => vec![e],
        None => DLock2Experiment::to_vec_ref(),
    };

    for experiment in experiments {
        let mut default_targets = None;

        let targets = option
            .lock_targets
            .as_ref()
            .unwrap_or_else(|| default_targets.insert(DLock2Target::iter().collect_vec()));

        let mut name_maybe = None;

        match experiment {
            DLock2Experiment::CounterProportional {
                cs_loops,
                non_cs_loops,
                file_name,
                include_lock_free,
                stat_hold_time,
            } => proportional_counter(
                bencher,
                file_name.as_deref().unwrap_or_else(|| {
                    name_maybe.insert(format!(
                        "counter cs {:?} noncs {:?}",
                        cs_loops, non_cs_loops
                    ))
                }),
                targets.iter(),
                cs_loops.iter().copied(),
                non_cs_loops.iter().copied(),
                *include_lock_free,
                *stat_hold_time,
            ),
            DLock2Experiment::FetchAndMultiply { include_lock_free } => {
                fetch_and_multiply(bencher, targets.iter(), *include_lock_free)
            }
            DLock2Experiment::Queue {
                lock_free_queues,
                seq_queue_type,
            } => match seq_queue_type {
                SeqQueueType::LinkedList => {
                    queue::benchmark_queue(bencher, LinkedList::new, targets.iter())
                }
                SeqQueueType::VecDeque => {
                    queue::benchmark_queue(bencher, VecDeque::new, targets.iter())
                }
            },
            DLock2Experiment::PriorityQueue { sequencial_pq_type } => match sequencial_pq_type {
                SeqPQType::BTreeSet => {
                    priority_queue::benchmark_pq(bencher, BTreeSet::new, targets.iter())
                }
                SeqPQType::BinaryHeap => {
                    priority_queue::benchmark_pq(bencher, BinaryHeap::new, targets.iter())
                }
                SeqPQType::PairingHeap => todo!(),
            },
        }
    }
}
