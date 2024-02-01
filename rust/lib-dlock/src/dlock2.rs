use enum_dispatch::enum_dispatch;
use trait_set::trait_set;

pub mod cc;
pub mod fc;
pub mod mutex;
pub mod spinlock;

pub trait DLock2Delegate<T> = Fn(&mut T, T) -> T + Send + Sync;

#[enum_dispatch]
pub trait DLock2<T, F>: Send + Sync
where
    F: DLock2Delegate<T>,
{
    fn lock(&self, data: T) -> T;
}
