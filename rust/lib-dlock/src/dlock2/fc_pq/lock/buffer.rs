use std::{
    cell::{SyncUnsafeCell, UnsafeCell},
    cmp::min,
    hint::spin_loop,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

use crossbeam::utils::CachePadded;

use crate::atomic_extension::AtomicExtension;

#[derive(Debug)]
pub struct ConcurrentRingBuffer<T, const N: usize> {
    pub buffer: SyncUnsafeCell<[MaybeUninit<T>; N]>,
    pub head: CachePadded<AtomicUsize>,
    pub tail: CachePadded<AtomicUsize>,
}

unsafe impl<T: Send, const N: usize> Sync for ConcurrentRingBuffer<T, N> {}

impl<T: 'static, const N: usize> Default for ConcurrentRingBuffer<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: 'static, const N: usize> ConcurrentRingBuffer<T, N> {
    pub fn new() -> Self {
        Self {
            buffer: unsafe { MaybeUninit::uninit().assume_init() },
            head: AtomicUsize::new(0).into(),
            tail: AtomicUsize::new(0).into(),
        }
    }

    pub fn push(&self, value: T) {
        // acquire a position
        let tail = self.tail.fetch_add(1, Ordering::AcqRel);
        let mut head = self.head.load_acquire();

        loop {
            // check if the buffer is full
            // if the buffer is full, spin until the buffer is not full
            if tail.wrapping_sub(head) >= N {
                loop {
                    head = self.head.load_acquire();
                    if tail.wrapping_sub(head) < N {
                        break;
                    }

                    spin_loop();
                }
            }

            // invariant: the current position is owned by the current thread
            // invariant: any previous value in this location should be already consumed

            unsafe {
                (*self.buffer.get())[tail % N].write(value);
            }

            return;
        }
    }

    /// Whether the buffer is empty
    pub fn empty(&self) -> bool {
        self.head.load_acquire() == self.tail.load_acquire()
    }

    /// Iterate over the buffer.
    /// The iterator will not be invalidated by concurrent insertions.
    /// Drop the iterator to allow further insertions
    pub fn iter(&self) -> BufferIterator<T, N> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);

        BufferIterator {
            buffer: self,
            head,
            limit: min(tail, head + N),
        }
    }
}

pub struct BufferIterator<'a, T, const N: usize> {
    buffer: &'a ConcurrentRingBuffer<T, N>,
    head: usize,
    limit: usize,
}

impl<'a, T, const N: usize> Iterator for BufferIterator<'a, T, N> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        // one scan at most
        if self.head == self.limit {
            return None;
        }

        let buffer = unsafe { &*self.buffer.buffer.get() };

        // take ownership of the item in the buffer
        let value = unsafe { buffer[self.head % N].as_ptr().read() };

        self.head += 1;

        Some(value)
    }
}

impl<'a, T, const N: usize> Drop for BufferIterator<'a, T, N> {
    fn drop(&mut self) {
        self.buffer.head.store_release(self.head);
    }
}
