use enum_dispatch::enum_dispatch;

pub mod fc;
pub mod mutex;
pub mod spinlock;

#[enum_dispatch]
pub trait DLock2<T, F>: Send + Sync
where
    F: Fn(&mut T, T) -> T + Send + Sync,
{
    fn lock(&self, data: T) -> T;
}

