use crate::spin_lock::RawSpinLock;

mod lock;
mod node;

pub type FCSL<T, I, F, L = RawSpinLock> = lock::FCSL<T, I, F, L>;
