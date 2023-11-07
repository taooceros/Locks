use std::{cell::SyncUnsafeCell, sync::Mutex};

use crate::dlock::DLockDelegate;
use crate::{dlock::DLock, guard::DLockGuard};

impl<T: Sized> DLock<T> for Mutex<T> {
    fn lock<'a>(&self, mut f: impl DLockDelegate<T> + 'a) {
        if let Ok(mut mutex_guard) = self.lock() {
            let data = &mut (*mutex_guard) as *mut T as *const SyncUnsafeCell<T>;
            unsafe {
                f.apply(DLockGuard::new(&*data));
            }
        }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<std::num::NonZeroI64> {
        return None;
    }
}
