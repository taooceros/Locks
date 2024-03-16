use std::collections::{BTreeSet, BinaryHeap};

pub trait SequentialPriorityQueue<T>
where
    T: PartialOrd + Ord + Eq,
{
    fn new() -> Self;
    fn push(&mut self, item: T);
    fn peek(&mut self) -> Option<&T>;
    fn pop(&mut self) -> Option<T>;
    fn len(&self) -> usize;
}

impl<T> SequentialPriorityQueue<T> for BinaryHeap<T>
where
    T: Ord,
{
    fn new() -> Self {
        BinaryHeap::new()
    }

    fn push(&mut self, item: T) {
        self.push(item);
    }

    fn peek(&mut self) -> Option<&T> {
        BinaryHeap::peek(self)
    }

    fn pop(&mut self) -> Option<T> {
        self.pop()
    }

    fn len(&self) -> usize {
        self.len()
    }
}

impl<T> SequentialPriorityQueue<T> for BTreeSet<T>
where
    T: Ord,
{
    fn new() -> Self {
        BTreeSet::new()
    }

    fn push(&mut self, item: T) {
        self.insert(item);
    }

    fn peek(&mut self) -> Option<&T> {
        self.first()
    }

    fn pop(&mut self) -> Option<T> {
        self.pop_first()
    }

    fn len(&self) -> usize {
        self.len()
    }
}
