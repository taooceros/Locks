use std::{
    cell::SyncUnsafeCell,
    marker::PhantomData,
    mem::transmute,
    ops::{Deref, DerefMut},
    ptr::Unique,
    sync::{
        atomic::{AtomicI32, Ordering::*, AtomicUsize},
        Arc,
    },
    thread::yield_now,
};

use super::{rclrequest::*, rclserver::*};

pub struct RclLock<T: Sized> {
    server: Unique<RclServer>,
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
            server: Unique::new(server).unwrap(),
            holder: AtomicUsize::new(0),
            data: SyncUnsafeCell::new(t),
        }
    }

    pub fn lock<'b>(&self, f: &'b mut (dyn FnMut(&mut RclGuard<T>) + 'b)) {
        let server = unsafe { &mut *self.server.as_ptr() };

        let client_id = server.client_id.with_init(|| {
            let id = server.num_clients.fetch_add(1, Relaxed);
            id
        });

        unsafe {
            let mut request: &mut RclRequest<T> = transmute(&mut (server.requests[*client_id]));

            request.lock = self;
            let real_me = client_id;
            request.real_me = *real_me;

            request.f = Some(f);

            while let Some(ref f) = request.f {
                yield_now();
            }
        }
    }
}
