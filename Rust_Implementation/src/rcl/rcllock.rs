use std::{
    cell::SyncUnsafeCell,
    marker::PhantomData,
    mem::transmute,
    ops::{Deref, DerefMut},
    ptr::Unique,
    sync::{
        atomic::{AtomicI32, AtomicUsize, Ordering::*},
        Arc,
    },
    thread::yield_now,
};

pub mod rcllockptr;
use super::{rclrequest::*, rclserver::*, syncptr::SyncMutPtr};
pub(crate) use rcllockptr::*;

pub struct RclLock<T: Sized> {
    server: SyncMutPtr<RclServer>,
    pub(super) holder: AtomicUsize,
    data: SyncUnsafeCell<T>,
}

pub struct RclGuard<'a, T: Sized> {
    lock: &'a RclLock<T>,
}

impl<'a, T: Sized> RclGuard<'a, T> {
    pub fn new(lock: &'a RclLock<T>) -> RclGuard<'a, T> {
        RclGuard { lock }
    }
}

impl<T: Sized> Deref for RclGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: Sized> DerefMut for RclGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> RclLock<T> {
    pub fn new<'a>(server: *mut RclServer, t: T) -> RclLock<T> {
        RclLock {
            server: server.into(),
            holder: AtomicUsize::new(!0),
            data: SyncUnsafeCell::new(t),
        }
    }

    pub fn lock<'b>(&self, f: &'b mut (dyn FnMut(&mut RclGuard<T>) + 'b)) {
        let serverptr: *mut RclServer = self.server.into();

        let server = unsafe { &mut *serverptr };

        let client_id = server.client_id.with_init(|| {
            let id = server.num_clients.fetch_add(1, Relaxed);
            id
        });

        let mut request: &mut RclRequest<T> =
            unsafe { transmute(&mut (server.requests[*client_id])) };

        request.lock = self.into();
        let real_me = client_id;
        request.real_me = *real_me;

        request.f = Some(unsafe { transmute(f) });

        while let Some(ref _f) = request.f {
            yield_now();
        }
    }
}
