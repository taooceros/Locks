use std::{
    cell::SyncUnsafeCell,
    mem::transmute,
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering::*},
    thread::yield_now,
};

pub mod rcllockptr;
use crate::{
    dlock::{DLock, DLockDelegate},
    syncptr::SyncMutPtr,
};

use super::{rclrequest::*, rclserver::*};
pub(crate) use rcllockptr::*;

pub struct RclLock<T: Sized> {
    server: SyncMutPtr<RclServer>,
    pub(super) holder: AtomicUsize,
    pub(super) data: SyncUnsafeCell<T>,
}

impl<T> DLock<T> for RclLock<T> {
    fn lock<'a>(&self, f: impl DLockDelegate<T> + 'a) {
        self.lock(f);
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

    pub fn lock<'a>(&self, mut f: impl DLockDelegate<T> + 'a) {
        let serverptr: *mut RclServer = self.server.into();

        let server = unsafe { &mut *serverptr };

        let client_id = server.client_id.get_or(|| {
            let id = server.num_clients.fetch_add(1, Relaxed);
            id
        });

        let request: &mut RclRequest<T> = unsafe { transmute(&mut (server.requests[*client_id])) };

        request.lock = self.into();
        let real_me = client_id;
        request.real_me = *real_me;

        unsafe {
            let f_request = &mut *(request.f.get());

            *f_request = Some(transmute(&mut f as &mut dyn DLockDelegate<T>));

            while f_request.is_some() {
                yield_now();
            }
        }
    }
}
