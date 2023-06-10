use std::{
    cell::SyncUnsafeCell,
    mem::transmute,
    sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicUsize, Ordering, Ordering::*},
    time::Duration, ptr::read_volatile,
};

use crossbeam::{
    epoch::{pin, Atomic, Guard, Owned, Shared},
    utils::{Backoff, CachePadded},
};
use thread_local::ThreadLocal;
use volatile::Volatile;

use crate::{
    dlock::DLock,
    guard::{self, *},
    syncptr::SyncMutPtr,
    RawSimpleLock,
};
use std::hint::spin_loop;

use linux_futex::Futex;

use self::record::*;

mod record;

pub struct FcLock2<T: Send + Sync, L: RawSimpleLock> {
    pass: SyncUnsafeCell<usize>,
    lock: CachePadded<L>,
    data: SyncUnsafeCell<T>,
    publications: Atomic<Record<T>>,
    thread_local: ThreadLocal<Atomic<Record<T>>>,
}

impl<T: Send + Sync, L: RawSimpleLock> FcLock2<T, L> {
    fn push_record(&self, record: Shared<Record<T>>, guard: &Guard) {
        let mut head = self.publications.load(Ordering::Acquire, guard);

        loop {
            unsafe {
                record.deref().next.store(head, Release);
            }

            let result = self
                .publications
                .compare_exchange_weak(head, record, Release, Relaxed, guard);

            match result {
                Ok(_) => {}
                Err(new_head) => head = new_head.current,
            }
        }
    }

    fn active_or_repush(&self, record: Shared<Record<T>>, guard: &Guard) {
        unsafe {
            if record.deref().state.load(Ordering::Acquire) {
                return;
            }

            self.push_record(record, guard);
        }
    }

    fn load_record<'a>(&self, guard: &'a Guard) -> Shared<'a, Record<T>> {
        let record = self.thread_local.get_or(|| {
            Atomic::new(Record {
                operation: SyncUnsafeCell::new(None),
                result: Atomic::null(),
                state: AtomicBool::new(false),
                age: AtomicUsize::new(0),
                next: Atomic::null(),
            })
        });

        let record = record.load(Ordering::Acquire, &guard);

        unsafe {
            if (!record.deref().state.load(Ordering::Acquire)) {
                self.push_record(record, guard);
            }
        }

        return record;
    }

    fn combine(&self, guard: &Guard) {
        let mut node = self.publications.load(Acquire, guard);

        while !node.is_null() {
            let node_ref = unsafe { node.deref() };

            if node_ref.result.load(Acquire, guard).tag() == 1 {
                let operation = unsafe { (*node_ref.operation.get()).take() };

                let result = operation.unwrap().call(&mut DLockGuard::new(&self.data));

                node_ref
                    .result
                    .store(Owned::new(result).with_tag(1), Release);

                node_ref.state.store(true, Release);
            }

            node = node_ref.next.load(Acquire, guard);
        }
    }

    fn clean_unactive_node(&self, guard: &Guard) {
        let mut node = self.publications.load(Acquire, guard);

        while !node.is_null() {
            let node_ref = unsafe { node.deref() };

            let next = node_ref.next.load(Acquire, guard);

            if !next.is_null() {
                let next_ref = unsafe { next.deref() };

                node_ref
                    .next
                    .store(next_ref.next.load(Acquire, guard), Release);

                node_ref.state.store(false, Release);
            } else {
                break;
            }

            node = node_ref.next.load(Acquire, guard);
        }
    }

    pub fn lock(&self, f: impl Callable<T> + 'static) {
        let guard = &pin();

        let record = self.load_record(guard);

        let record_ref = unsafe { record.deref() };

        unsafe {
            *record_ref.operation.get() = Some(Box::new(f));
        }

        record_ref.result.store(Shared::null().with_tag(1), Release);

        let backoff = Backoff::new();

        if self.lock.try_lock() {
            self.combine(guard);

            unsafe{
                if read_volatile(self.pass.get()) % 50 == 0{
                    self.clean_unactive_node(guard);
                }
            }

        } else {
            while record_ref.result.load(Acquire, guard).tag() == 0 {
                backoff.snooze();

                self.active_or_repush(record, &guard);
            }
        }
    }
}
