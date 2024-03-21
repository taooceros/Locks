use std::sync::Mutex;

use crate::spin_lock::RawSpinLock;

mod lock;
mod node;

pub type FC<T, I, F, L = RawSpinLock> = lock::FC<T, I, F, L>;
