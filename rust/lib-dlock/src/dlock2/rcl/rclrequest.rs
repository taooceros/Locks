use std::sync::atomic::AtomicPtr;

use super::rcllock::RCLLock;

#[repr(C)]
pub struct RclRequest<T, P> {
    pub(super) real_me: usize,
    pub(super) lock: AtomicPtr<RCLLock<'static, T>>,
    pub(super) parker: P,
    pub(super) data: T,
}
