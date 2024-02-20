use std::collections::LinkedList;

use std::collections::VecDeque;

use clap::ValueEnum;
use libdlock::dlock2::DLock2;
use strum::Display;
use strum::EnumIter;

#[derive(Debug, Clone, ValueEnum, EnumIter, Display)]
pub enum LockFreeQueue {
    MCSQueue,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum QueueData<T: Send> {
    #[default]
    Nothing,
    Push {
        data: T,
    },
    Pop,
    OutputT {
        data: T,
    },
    OutputEmpty,
}

pub unsafe trait ConcurrentQueue<T>: Send + Sync
where
    T: Send,
{
    fn push(&self, value: T);
    fn pop(&self) -> Option<T>;
}

pub trait SequentialQueue<T> {
    fn push(&mut self, value: T);
    fn pop(&mut self) -> Option<T>;
}

unsafe impl<T, L> ConcurrentQueue<T> for L
where
    T: Send,
    L: DLock2<QueueData<T>>,
{
    fn push(&self, value: T) {
        self.lock(QueueData::Push { data: value });
    }

    fn pop(&self) -> Option<T> {
        match self.lock(QueueData::Pop) {
            QueueData::OutputT { data } => Some(data),
            QueueData::OutputEmpty => None,
            _ => panic!("Invalid output"),
        }
    }
}

impl<T: Send> SequentialQueue<T> for VecDeque<T> {
    fn push(&mut self, value: T) {
        self.push_back(value);
    }

    fn pop(&mut self) -> Option<T> {
        self.pop_front()
    }
}

impl<T: Send> SequentialQueue<T> for LinkedList<T> {
    fn push(&mut self, value: T) {
        self.push_back(value);
    }

    fn pop(&mut self) -> Option<T> {
        self.pop_front()
    }
}
