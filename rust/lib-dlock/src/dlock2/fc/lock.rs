use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    ptr::{self, null_mut, NonNull},
    sync::atomic::{AtomicPtr, AtomicU32, Ordering::*},
};

use crossbeam::utils::{Backoff, CachePadded};
use lock_api::RawMutex;
use thread_local::ThreadLocal;

use crate::dlock2::{DLock2, DLock2Delegate};

use super::node::Node;

const CLEAN_UP_AGE: u32 = 500;

#[derive(Debug)]
pub struct FC<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    F: Fn(&mut T, I) -> I,
    L: RawMutex,
{
    pass: AtomicU32,
    combiner_lock: CachePadded<L>,
    delegate: F,
    data: SyncUnsafeCell<T>,
    head: AtomicPtr<Node<I>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<I>>>,
}

impl<T, I, F, L> FC<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
    L: RawMutex,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            pass: AtomicU32::new(0),
            combiner_lock: CachePadded::new(L::INIT),
            delegate,
            data: SyncUnsafeCell::new(data),
            head: AtomicPtr::new(std::ptr::null_mut()),
            local_node: ThreadLocal::new(),
        }
    }

    fn push_node(&self, node: &mut Node<I>) {
        let mut head = self.head.load(Acquire);
        node.active.store(true, Release);
        loop {
            node.next.store(head, Relaxed);
            match self
                .head
                .compare_exchange_weak(head, node, Release, Acquire)
            {
                Ok(_) => {
                    break;
                }
                Err(x) => head = x,
            }
        }
    }

    fn push_if_unactive(&self, node: &mut Node<I>) {
        if node.active.load(Acquire) {
            return;
        }
        self.push_node(node);
    }

    fn combine(&self) {
        let mut current_ptr = NonNull::new(self.head.load(Acquire));

        let pass = self.pass.fetch_add(1, Relaxed);

        #[cfg(feature = "combiner_stat")]
        let mut aux: u32 = 0;
        #[cfg(feature = "combiner_stat")]
        let begin: u64;

        #[cfg(feature = "combiner_stat")]
        unsafe {
            begin = __rdtscp(&mut aux);
        }

        while let Some(current_nonnull) = current_ptr {
            let current = unsafe { current_nonnull.as_ref() };

            if current.active.load(Acquire) && !current.complete.load(Acquire) {
                unsafe {
                    (*current.age.get()) = pass;
                    current.data.get().write(MaybeUninit::new((self.delegate)(
                        self.data.get().as_mut().unwrap_unchecked(),
                        current.data.get().read().assume_init(),
                    )));
                }

                current.complete.store(true, Release);
            }

            current_ptr = NonNull::new(current.next.load(Acquire));
        }

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            (*self.local_node.get().unwrap().get()).combiner_time_stat += end - begin;
        }
    }

    unsafe fn clean_unactive_node(&self, head: &AtomicPtr<Node<I>>, pass: u32) {
        let previous_ptr = NonNull::new(head.load(Acquire)).unwrap();

        let mut previous_nonnull = previous_ptr;

        let mut current_ptr = NonNull::new(previous_nonnull.as_ref().next.load(Acquire));

        while let Some(current_nonnull) = current_ptr {
            let current = current_nonnull.as_ref();
            let previous = previous_nonnull.as_ref();

            // assert!(current.active.load(Acquire));

            if pass - (*current.age.get()) > CLEAN_UP_AGE {
                previous.next.store(current.next.load(Acquire), Release);
                current.next.store(null_mut(), Release);
                current.active.store(false, Release);
                current_ptr = NonNull::new(previous.next.load(Acquire));
                continue;
            }

            previous_nonnull = current_nonnull;
            current_ptr = NonNull::new(current.next.load(Acquire));
        }
    }
}

unsafe impl<'a, T, I, F, L> DLock2<I> for FC<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
    L: RawMutex + Send + Sync,
{
    fn lock(&self, data: I) -> I {
        let node = self.local_node.get_or(|| SyncUnsafeCell::new(Node::new()));

        let node = unsafe { &mut *node.get() };

        node.data = SyncUnsafeCell::new(MaybeUninit::new(data));
        node.complete.store(false, Release);

        'outer: loop {
            self.push_if_unactive(node);

            if self.combiner_lock.try_lock() {
                self.combine();
                unsafe {
                    let pass = self.pass.load(Relaxed);

                    if pass % CLEAN_UP_AGE == 0 {
                        self.clean_unactive_node(&self.head, pass);
                    }

                    self.combiner_lock.unlock();
                }

                if node.complete.load(Acquire) {
                    break 'outer;
                }
            } else {
                let backoff = Backoff::new();
                let mut count: u32 = 8;
                loop {
                    if node.complete.load(Acquire) {
                        break 'outer;
                    }
                    backoff.spin();
                    count = count.wrapping_sub(1);
                    if count == 0 {
                        continue 'outer;
                    }
                }
            }
        }

        unsafe { node.data.get().read().assume_init() }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        unsafe { self.local_node.get().map(|x| (*x.get()).combiner_time_stat) }
    }
}
