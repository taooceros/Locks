use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    ops::{AddAssign, SubAssign},
    ptr::{self, null_mut, NonNull},
    sync::atomic::{AtomicPtr, Ordering::*},
};

use crate::dlock2::combiner_stat::CombinerSample;
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
    pass: SyncUnsafeCell<u32>,
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
            pass: SyncUnsafeCell::new(0),
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

        let pass = unsafe { self.pass.get().read() };
        unsafe {
            self.pass.get().write(pass + 1);
        }

        #[cfg(feature = "combiner_stat")]
        let mut aux: u32 = 0;
        #[cfg(feature = "combiner_stat")]
        let begin: u64;

        #[cfg(feature = "combiner_stat")]
        let mut count = 0;

        unsafe {
            begin = __rdtscp(&mut aux);
        }

        while let Some(current_nonnull) = current_ptr {
            let current = unsafe { current_nonnull.as_ref() };

            if current.active.load(Acquire) && !current.complete.load(Acquire) {
                unsafe {
                    (*current.age.get()) = pass;
                    *current.data.get() = (self.delegate)(
                        self.data.get().as_mut().unwrap_unchecked(),
                        ptr::read(current.data.get()),
                    )
                    .into();
                }

                current.complete.store(true, Release);

                count += 1;
            }

            current_ptr = NonNull::new(current.next.load(Acquire));
        }

        unsafe {
            self.local_node
                .get()
                .unwrap()
                .get()
                .as_mut()
                .unwrap()
                .combiner_stat
                .insert_sample(begin, count);
        }
    }

    unsafe fn clean_unactive_node(&self, head: &AtomicPtr<Node<I>>, pass: u32) {
        let previous_ptr = NonNull::new(head.load(Acquire)).unwrap();

        let mut previous_nonnull = previous_ptr;

        let mut current_ptr = NonNull::new(*previous_nonnull.as_ref().next.as_ptr());

        while let Some(current_nonnull) = current_ptr {
            let current = current_nonnull.as_ref();
            let previous = previous_nonnull.as_ref();

            // assert!(current.active.load(Acquire));

            if pass - (*current.age.get()) > CLEAN_UP_AGE {
                (*previous.next.as_ptr()) = *current.next.as_ptr();
                (*current.next.as_ptr()) = null_mut();
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

        node.data = data.into();
        node.complete.store(false, Release);

        'outer: loop {
            self.push_if_unactive(node);

            if self.combiner_lock.try_lock() {
                self.combine();
                unsafe {
                    let pass = self.pass.get().read();

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
                let mut count = 8;
                loop {
                    if node.complete.load(Acquire) {
                        break 'outer;
                    }
                    backoff.spin();
                    count -= 1;
                    if count < 0 {
                        continue 'outer;
                    }
                }
            }
        }

        unsafe { ptr::read(node.data.get()) }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_stat(&self) -> Option<&CombinerSample> {
        unsafe { self.local_node.get().map(|x| &(*x.get()).combiner_stat) }
    }
}
