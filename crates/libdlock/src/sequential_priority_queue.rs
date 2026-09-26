use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};
// Only these adapters may store UsageNode<'static, _>: its reference is valid
// only while the owning FCPQ is alive. A safe custom queue could copy it out.
mod sealed {
    use super::{BTreeSet, BinaryHeap, Reverse};

    pub trait Sealed {}
    impl<T: Ord> Sealed for BinaryHeap<Reverse<T>> {}
    impl<T: Ord> Sealed for BTreeSet<T> {}
}

/// The built-in queues keep their entries inside the owning lock.
///
/// A downstream queue cannot implement this trait, even if it implements the
/// public methods: doing so could leak the lock's internally borrowed nodes.
///
/// ```compile_fail
/// use libdlock::sequential_priority_queue::SequentialPriorityQueue;
/// struct Custom;
/// impl SequentialPriorityQueue<u64> for Custom {
///     fn new() -> Self { Custom }
///     fn push(&mut self, _: u64) {}
///     fn peek(&mut self) -> Option<&u64> { None }
///     fn pop(&mut self) -> Option<u64> { None }
///     fn len(&self) -> usize { 0 }
/// }
/// ```
pub trait SequentialPriorityQueue<T>: sealed::Sealed
where
    T: PartialOrd + Ord + Eq,
{
    fn new() -> Self;
    fn push(&mut self, item: T);
    fn peek(&mut self) -> Option<&T>;
    fn pop(&mut self) -> Option<T>;
    fn len(&self) -> usize;
}

impl<T> SequentialPriorityQueue<T> for BinaryHeap<Reverse<T>>
where
    T: Ord,
{
    fn new() -> Self {
        BinaryHeap::new()
    }

    fn push(&mut self, item: T) {
        self.push(Reverse(item));
    }

    fn peek(&mut self) -> Option<&T> {
        BinaryHeap::peek(self).map(|r| &r.0)
    }

    fn pop(&mut self) -> Option<T> {
        self.pop().map(|r| r.0)
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
