use std::{
    cell::{SyncUnsafeCell, UnsafeCell},
    cmp::min,
    hint::spin_loop,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

use crossbeam::utils::{Backoff, CachePadded};

use crate::atomic_extension::AtomicExtension;

#[derive(Debug)]
pub struct ConcurrentRingBuffer<T, const N: usize> {
    pub buffer: SyncUnsafeCell<[MaybeUninit<Entry<T>>; N]>,
    pub head: CachePadded<AtomicUsize>,
    pub tail: CachePadded<AtomicUsize>,
}

struct Entry<T> {
    value: SyncUnsafeCell<T>,
    valid: CachePadded<AtomicUsize>,
}

unsafe impl<T: Send, const N: usize> Sync for ConcurrentRingBuffer<T, N> {}

impl<T: 'static, const N: usize> Default for ConcurrentRingBuffer<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: 'static, const N: usize> ConcurrentRingBuffer<T, N> {
    pub fn new() -> Self {
        let mut buffer = Self {
            buffer: unsafe { MaybeUninit::uninit().assume_init() },
            head: AtomicUsize::new(0).into(),
            tail: AtomicUsize::new(0).into(),
        };

        unsafe {
            for entry in buffer.buffer.get_mut().iter_mut() {
                entry.write(Entry {
                    value: MaybeUninit::uninit().assume_init(),
                    valid: AtomicUsize::new(0).into(),
                });
            }
        }

        buffer
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
                let entry = (*self.buffer.get())[tail % N].assume_init_mut();

                let backoff = Backoff::new();

                // 1 if the entry is used by the other thread
                while entry.valid.load_acquire() != 0 {
                    backoff.snooze();
                }

                entry.value.get().write(value);
                entry.valid.store_release(1);
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
    /// Only one thread can hold the iterator at a time
    pub unsafe fn iter(&self) -> BufferIterator<T, N> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

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

    fn size_hint(&self) -> (usize, Option<usize>) {
        let size = self.limit - self.head;
        (size, Some(size))
    }

    fn next(&mut self) -> Option<Self::Item> {
        // one scan at most
        if self.head == self.limit {
            return None;
        }

        let buffer = unsafe { &*self.buffer.buffer.get() };

        let entry = unsafe { buffer[self.head % N].assume_init_ref() };

        while entry.valid.load_acquire() == 0 {
            spin_loop();
        }

        // take ownership of the item in the buffer
        let value = unsafe { entry.value.get().read() };

        self.head += 1;

        entry.valid.store_release(0);

        Some(value)
    }
}

impl<'a, T, const N: usize> Drop for BufferIterator<'a, T, N> {
    fn drop(&mut self) {
        self.buffer.head.store_release(self.head);
    }
}
