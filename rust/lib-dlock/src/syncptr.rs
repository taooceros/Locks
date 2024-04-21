#[derive(Debug)]
pub struct SyncMutPtr<T: ?Sized> {
    pub ptr: *mut T,
}

impl<T> Clone for SyncMutPtr<T> {
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr.clone(),
        }
    }
}
impl<T> Copy for SyncMutPtr<T> {}

unsafe impl<T: Sync> Sync for SyncMutPtr<T> {}
unsafe impl<T: Send> Send for SyncMutPtr<T> {}

impl<T> From<*mut T> for SyncMutPtr<T> {
    fn from(ptr: *mut T) -> Self {
        Self { ptr }
    }
}

impl<T> From<&mut T> for SyncMutPtr<T> {
    fn from(ptr: &mut T) -> Self {
        Self { ptr }
    }
}

impl<T> Into<*mut T> for SyncMutPtr<T> {
    fn into(self) -> *mut T {
        self.ptr
    }
}

impl<T> Into<*const T> for SyncMutPtr<T> {
    fn into(self) -> *const T {
        self.ptr
    }
}


#[derive(Debug, Clone, Copy)]
pub struct SyncPtr<T: ?Sized> {
    ptr: *const T,
}

unsafe impl<T: Sync> Sync for SyncPtr<T> {}
unsafe impl<T: Send> Send for SyncPtr<T> {}

impl<T> From<*const T> for SyncPtr<T> {
    fn from(ptr: *const T) -> Self {
        Self { ptr }
    }
}

impl<T> From<&T> for SyncPtr<T> {
    fn from(ptr: &T) -> Self {
        Self { ptr }
    }
}

impl<T> Into<*mut T> for SyncPtr<T> {
    fn into(self) -> *mut T {
        self.ptr as *mut T
    }
}
