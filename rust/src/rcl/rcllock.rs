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
    parker::Parker,
    syncptr::SyncMutPtr,
};

use super::{rclrequest::*, rclserver::*};
pub(crate) use rcllockptr::*;

#[derive(Debug)]
pub struct RclLock<T, P>
where
    T: Sized,
    P: Parker + 'static,
{
    server: SyncMutPtr<RclServer<P>>,
    pub(super) holder: AtomicUsize,
    pub(super) data: SyncUnsafeCell<T>,
}

impl<T, P: Parker> DLock<T> for RclLock<T, P> {
    fn lock<'a>(&self, f: impl DLockDelegate<T> + 'a) {
        self.lock(f);
    }

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<std::num::NonZeroI64> {
        return None;
    }
}

impl<T, P: Parker> RclLock<T, P> {
    pub fn new<'a>(server: *mut RclServer<P>, t: T) -> RclLock<T, P> {
        RclLock {
            server: server.into(),
            holder: AtomicUsize::new(!0),
            data: SyncUnsafeCell::new(t),
        }
    }

    pub fn lock<'a>(&self, mut f: impl DLockDelegate<T> + 'a) {
        let serverptr: *mut RclServer<P> = self.server.into();

        let server = unsafe { &mut *serverptr };

        let client_id = server.client_id.get_or(|| {
            let id = server.num_clients.fetch_add(1, Relaxed);
            id
        });

        let request: &mut RclRequest<T, P> =
            unsafe { transmute(&mut (server.requests[*client_id])) };

        request.lock = self.into();
        let real_me = client_id;
        request.real_me = *real_me;

        unsafe {
            let f_request = &mut *(request.f.get());


            *f_request = Some(transmute(&mut f as &mut dyn DLockDelegate<T>));

            request.parker.reset();
            request.parker.wait();
        }
    }
}
