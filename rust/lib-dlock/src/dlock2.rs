use enum_dispatch::enum_dispatch;

pub mod cc;
pub mod cc_ban;
pub mod fc;
pub mod fc_ban;
pub mod rcl;

pub mod mutex;
pub mod spinlock;
pub mod uscl;

pub trait DLock2Delegate<T> = Fn(&mut T, T) -> T + Send + Sync;

#[enum_dispatch]
pub trait DLock2<T, F>: Send + Sync
where
    F: DLock2Delegate<T>,
{
    fn lock(&self, data: T) -> T;
}
