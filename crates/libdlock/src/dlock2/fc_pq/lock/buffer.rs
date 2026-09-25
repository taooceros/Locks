use std::{
    cell::SyncUnsafeCell,
    cmp::min,
    hint::spin_loop,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

use crossbeam::utils::{Backoff, CachePadded};

use crate::atomic_extension::AtomicExtension;

#[derive(Debug)]
pub struct ConcurrentRingBuffer<T, const N: usize> {
    buffer: SyncUnsafeCell<[MaybeUninit<Entry<T>>; N]>,
    pub head: CachePadded<AtomicUsize>,
    pub tail: CachePadded<AtomicUsize>,
}

struct Entry<T> {
    value: SyncUnsafeCell<MaybeUninit<T>>,
    valid: CachePadded<AtomicUsize>,
}

// SAFETY: new() initializes all entries before sharing the buffer. Each
// ticket has one producer, there is one iterator, and valid transfers the
// interior value with release/acquire; head/tail/valid are atomic.
unsafe impl<T: Send, const N: usize> Sync for ConcurrentRingBuffer<T, N> {}

impl<T, const N: usize> ConcurrentRingBuffer<T, N> {
    // SAFETY: new() initializes every Entry before Self escapes. Its address
    // remains stable; concurrent users share the entry and touch its value only
    // through SyncUnsafeCell, with valid's release/acquire ownership handoff.
    unsafe fn entry(&self, index: usize) -> &Entry<T> {
        &*self.buffer.get().cast::<Entry<T>>().add(index)
    }
}

impl<T: 'static, const N: usize> Default for ConcurrentRingBuffer<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: 'static, const N: usize> ConcurrentRingBuffer<T, N> {
    pub fn new() -> Self {
        let mut buffer = Self {
            buffer: SyncUnsafeCell::new([const { MaybeUninit::uninit() }; N]),
            head: AtomicUsize::new(0).into(),
            tail: AtomicUsize::new(0).into(),
        };

        // Initialize each entry's `valid` flag to 0 (value stays uninitialized).
        for entry in buffer.buffer.get_mut().iter_mut() {
            entry.write(Entry {
                value: SyncUnsafeCell::new(MaybeUninit::uninit()),
                valid: AtomicUsize::new(0).into(),
            });
        }

        buffer
    }

    pub fn push(&self, value: T) {
        self.push_after_reserving(value, || {});
    }

    // The production push and the controlled reservation-gap regression use
    // this same publication path. The production hook is an inline no-op.
    fn push_after_reserving(&self, value: T, after_reserving: impl FnOnce()) {
        // acquire a position
        let tail = self.tail.fetch_add(1, Ordering::AcqRel);
        after_reserving();
        let mut head = self.head.load_acquire();

        // Spin until the buffer has space
        if tail.wrapping_sub(head) >= N {
            loop {
                head = self.head.load_acquire();
                if tail.wrapping_sub(head) < N {
                    break;
                }

                spin_loop();
            }
        }

        // Only this ticket's value may be written, but the consumer may
        // already be polling this same entry's valid flag through &Entry.
        unsafe {
            let entry = self.entry(tail % N);

            let backoff = Backoff::new();

            // 1 if the entry is used by the other thread
            while entry.valid.load_acquire() != 0 {
                backoff.snooze();
            }

            (*entry.value.get()).write(value);
            entry.valid.store_release(1);
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

        // SAFETY: entries were initialized before publication of the buffer.
        // The producer and consumer hold only shared references to each slot;
        // valid's acquire observes the producer's initialized value.
        let entry = unsafe { self.buffer.entry(self.head % N) };

        while entry.valid.load_acquire() == 0 {
            spin_loop();
        }

        // SAFETY: acquire of valid=1 transfers this slot's initialized value
        // to the sole iterator. Release of valid=0 below permits slot reuse.
        let value = unsafe { (*entry.value.get()).assume_init_read() };

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering::*},
        mpsc, Arc,
    };
    use std::thread;

    struct Owned {
        id: usize,
        drops: Arc<Vec<AtomicUsize>>,
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            self.drops[self.id].fetch_add(1, SeqCst);
        }
    }

    #[test]
    fn reserved_slot_shared_poll_and_reuse() {
        let ring = Arc::new(ConcurrentRingBuffer::<Owned, 2>::new());
        let drops = Arc::new((0..4).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>());
        let (reserved_tx, reserved_rx) = mpsc::channel();
        let (publish_tx, publish_rx) = mpsc::channel();
        let producer_ring = ring.clone();
        let producer_drops = drops.clone();
        let producer = thread::spawn(move || {
            // Pause the real push path just after reserving ticket 0. Its
            // publication below is the same code executed by push().
            producer_ring.push_after_reserving(
                Owned {
                    id: 0,
                    drops: producer_drops,
                },
                || {
                    reserved_tx.send(()).unwrap();
                    publish_rx.recv().unwrap();
                },
            );
        });
        reserved_rx.recv().unwrap();
        ring.push(Owned {
            id: 1,
            drops: drops.clone(),
        });

        let (polling_tx, polling_rx) = mpsc::channel();
        let consumer_ring = ring.clone();
        let consumer = thread::spawn(move || {
            // SAFETY: exactly one iterator; both tickets were reserved before
            // iter(), and both producers publish before next() returns.
            let mut iter = unsafe { consumer_ring.iter() };
            let shared_slot = unsafe { consumer_ring.entry(0) };
            polling_tx.send(()).unwrap();
            let first = iter.next().unwrap();
            let second = iter.next().unwrap();
            assert_eq!((first.id, second.id), (0, 1));
            drop((first, second));
            assert_eq!(shared_slot.valid.load_acquire(), 0);
            drop(iter);
        });
        polling_rx.recv().unwrap();
        publish_tx.send(()).unwrap();
        producer.join().unwrap();
        consumer.join().unwrap();

        // Wrap around the same two entries; the first two values must have
        // been moved out and dropped exactly once before reuse.
        for id in 2..4 {
            ring.push(Owned {
                id,
                drops: drops.clone(),
            });
        }
        let mut iter = unsafe { ring.iter() };
        for id in 2..4 {
            let item = iter.next().unwrap();
            assert_eq!(item.id, id);
            drop(item);
        }
        assert!(iter.next().is_none());
        drop(iter);
        drop(ring);
        for (id, count) in drops.iter().enumerate() {
            assert_eq!(count.load(SeqCst), 1, "slot value {id} ownership");
        }
    }
}
