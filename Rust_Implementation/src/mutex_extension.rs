use std::{
    cell::SyncUnsafeCell,
    sync::Mutex,
};

use crate::{dlock::DLock, guard::Guard};

impl<T: Sized> DLock<T> for Mutex<T> {
    fn lock<'b>(&self, f: &mut (dyn FnMut(&mut Guard<T>) + 'b)) {
        if let Ok(mut mutex_guard) = self.lock() {
            let data = &mut (*mutex_guard) as *mut T as *const SyncUnsafeCell<T>;
            unsafe {
                let mut guard = Guard::new(&*data);
                f(&mut guard);
            }
        }
    }
}
