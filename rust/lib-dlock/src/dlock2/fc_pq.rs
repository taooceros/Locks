use crate::spin_lock::RawSpinLock;

mod lock;
mod node;

pub type FCPQ<T, I, PQ, F, L = RawSpinLock> = lock::FCPQ<T, I, PQ, F, L>;
pub type UsageNode<'a, I> = lock::UsageNode<'a, I>;
