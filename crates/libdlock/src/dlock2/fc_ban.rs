use crate::spin_lock::RawSpinLock;

mod lock;
mod node;

pub type FCBan<T, I, F, L = RawSpinLock> = lock::FCBan<T, I, F, L>;
