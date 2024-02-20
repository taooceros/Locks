use std::{
    self,
    collections::{BTreeSet, BinaryHeap},
};

use libdlock::dlock2::DLock2;

use crossbeam_skiplist::SkipSet;

pub unsafe trait ConcurrentPriorityQueue<T>: Send + Sync
where
    T: PartialOrd + Ord + Eq + Send + Sync,
{
    fn push(&self, item: T);
    fn peek(&self) -> Option<T>;
    fn pop(&self) -> Option<T>;
}

unsafe impl<T> ConcurrentPriorityQueue<T> for SkipSet<T>
where
    T: PartialOrd + Ord + Eq + Send + Sync + Copy + 'static,
{
    fn push(&self, item: T) {
        self.insert(item);
    }

    fn peek(&self) -> Option<T> {
        self.front().map(|x| *x)
    }
    fn pop(&self) -> Option<T> {
        self.pop_front().map(|x| *x)
    }
}

pub trait SequentialPriorityQueue<T>
where
    T: PartialOrd + Ord + Eq,
{
    fn push(&mut self, item: T);
    fn peek(&mut self) -> Option<&T>;
    fn pop(&mut self) -> Option<T>;
}

pub struct DLock2PriorityQueue<'a, T, Q, L>
where
    T: PartialOrd + Ord + Eq + Send + Sync + 'a,
    Q: SequentialPriorityQueue<T>,
    L: DLock2<PQData<T>>,
{
    pub(crate) inner: L,
    pub(crate) phantom: std::marker::PhantomData<T>,
    pub(crate) phantom2: std::marker::PhantomData<Q>,
    phantom3: std::marker::PhantomData<&'a ()>,
}

impl<'a, T, Q, L> DLock2PriorityQueue<'a, T, Q, L>
where
    T: PartialOrd + Ord + Eq + Send + Sync,
    Q: SequentialPriorityQueue<T>,
    L: DLock2<PQData<T>>,
{
    pub fn new(inner: L) -> Self {
        DLock2PriorityQueue {
            inner,
            phantom: std::marker::PhantomData,
            phantom2: std::marker::PhantomData,
            phantom3: std::marker::PhantomData,
        }
    }
}

unsafe impl<'a, T, Q, L> Send for DLock2PriorityQueue<'a, T, Q, L>
where
    T: PartialOrd + Ord + Eq + Send + Sync,
    Q: SequentialPriorityQueue<T>,
    L: DLock2<PQData<T>>,
{
}

unsafe impl<'a, T, Q, L> Sync for DLock2PriorityQueue<'a, T, Q, L>
where
    T: PartialOrd + Ord + Eq + Send + Sync,
    Q: SequentialPriorityQueue<T>,
    L: DLock2<PQData<T>>,
{
}

#[derive(Debug, Default)]
pub(crate) enum PQData<T> {
    #[default]
    Nothing,
    Push {
        data: T,
    },
    Pop,
    Peek,
    PeekResult(Option<T>),
    PopResult(Option<T>),
}

unsafe impl<'a, T, Q, L> ConcurrentPriorityQueue<T> for DLock2PriorityQueue<'a, T, Q, L>
where
    T: PartialOrd + Ord + Eq + Send + Sync,
    Q: SequentialPriorityQueue<T>,
    L: DLock2<PQData<T>>,
{
    fn push(&self, item: T) {
        self.inner.lock(PQData::Push { data: item });
    }

    fn pop(&self) -> Option<T> {
        if let PQData::PopResult(result) = self.inner.lock(PQData::Pop) {
            result
        } else {
            panic!("DLock2PriorityQueue::pop: unexpected result");
        }
    }

    fn peek(&self) -> Option<T> {
        if let PQData::PeekResult(item) = self.inner.lock(PQData::Peek) {
            item
        } else {
            panic!("DLock2PriorityQueue::peek: unexpected result");
        }
    }
}

impl<T> SequentialPriorityQueue<T> for BinaryHeap<T>
where
    T: Ord,
{
    fn push(&mut self, item: T) {
        self.push(item);
    }

    fn peek(&mut self) -> Option<&T> {
        BinaryHeap::peek(self)
    }

    fn pop(&mut self) -> Option<T> {
        self.pop()
    }
}

impl<T> SequentialPriorityQueue<T> for BTreeSet<T>
where
    T: Ord,
{
    fn push(&mut self, item: T) {
        self.insert(item);
    }

    fn peek(&mut self) -> Option<&T> {
        self.first()
    }

    fn pop(&mut self) -> Option<T> {
        self.pop_first()
    }
}
