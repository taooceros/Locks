use crate::dlock2::{CombinerStatistics, DLock2};
use std::{
    arch::x86_64::__rdtscp,
    cell::SyncUnsafeCell,
    cmp::max,
    ops::AddAssign,
    ptr::{self, NonNull},
    sync::{
        atomic::{AtomicI64, AtomicPtr, AtomicU64, Ordering::*},
        Arc,
    },
};

use crossbeam::utils::Backoff;
use thread_local::ThreadLocal;

use super::node::Node;
use crate::dlock2::DLock2Delegate;

#[derive(Debug)]
pub struct ThreadData<T> {
    pub(crate) node: AtomicPtr<Node<T>>,
    pub(crate) banned_until: SyncUnsafeCell<u64>,
    #[cfg(feature = "combiner_stat")]
    pub combiner_time_stat: SyncUnsafeCell<CombinerStatistics>,
}

#[derive(Debug, Default)]
pub struct CCBan<T, I, F>
where
    F: DLock2Delegate<T, I>,
{
    delegate: F,
    data: SyncUnsafeCell<T>,
    tail: AtomicPtr<Node<I>>,
    avg_cs: SyncUnsafeCell<i64>,
    num_exec: SyncUnsafeCell<i64>,
    num_waiting_threads: AtomicU64,
    local_node: ThreadLocal<ThreadData<I>>,
}

impl<T, I, F> CCBan<T, I, F>
where
    F: DLock2Delegate<T, I>,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: SyncUnsafeCell::new(data),
            tail: AtomicPtr::new(Box::leak(Box::new(Node::default()))),
            local_node: ThreadLocal::new(),
            avg_cs: SyncUnsafeCell::new(0),
            num_exec: SyncUnsafeCell::new(0),
            num_waiting_threads: AtomicU64::new(0),
        }
    }

    fn ban(&self, data: &ThreadData<I>, panelty: u64) {
        unsafe {
            // println!(
            //     "current cs {}; avg cs {}",
            //     current_cs,
            //     self.avg_cs.load_consume()
            // );

            data.banned_until
                .get()
                .as_mut()
                .unwrap_unchecked()
                .add_assign(panelty);
        }
    }
}

const H: usize = 16;

unsafe impl<T, I, F> DLock2<I> for CCBan<T, I, F>
where
    T: Send + Sync,
    F: DLock2Delegate<T, I>,
{
    fn lock(&self, data: I) -> I {
        let thread_data = self.local_node.get_or(|| {
            self.num_waiting_threads.fetch_add(1, Relaxed);

            let current_tsc = unsafe {
                let mut aux = 0;
                __rdtscp(&mut aux)
            };

            ThreadData {
                node: AtomicPtr::new(Box::leak(Box::new(Node::default()))),
                banned_until: current_tsc.into(),
                combiner_time_stat: CombinerStatistics::default().into(),
            }
        });

        let mut aux = 0;

        unsafe {
            let banned_until = thread_data.banned_until.get().read();

            let backoff = Backoff::default();
            loop {
                let current = __rdtscp(&mut aux);

                if current >= banned_until {
                    break;
                }
                backoff.snooze();
            }
        }

        let mut aux = 0;
        // use thread local node as next node
        let next_node = unsafe { &mut *thread_data.node.load(Acquire) };

        next_node.next.store(std::ptr::null_mut(), Release);
        next_node.wait.store(true, Release);
        next_node.completed.store(false, Release);

        let current_ptr = self.tail.swap(next_node, AcqRel);
        let current_node = unsafe { current_ptr.as_ref().unwrap_unchecked() };

        unsafe {
            *current_node.data.get() = data;
            current_node.next.store(next_node, Release);
            self.local_node
                .get()
                .unwrap_unchecked()
                .node
                .store(current_ptr, Relaxed)
        }

        let backoff = Backoff::new();

        // wait for the current node to be waked
        while current_node.wait.load(Acquire) {
            // spin
            backoff.snooze();
        }

        // check whether the current node is completed
        if current_node.completed.load(Acquire) {
            unsafe {
                self.ban(thread_data, current_node.panelty.get().read());
            }
            return unsafe { ptr::read(current_node.data.get()) };
        }

        // combiner

        #[cfg(feature = "combiner_stat")]
        let begin = unsafe { __rdtscp(&mut aux) };

        let mut tmp_node = current_node;

        let mut counter: usize = 0;

        let mut next_ptr = NonNull::new(tmp_node.next.load(Acquire));

        let mut work_begin = begin;

        while let Some(next_nonnull) = next_ptr {
            if counter >= H {
                break;
            }

            counter += 1;
            let next_node = unsafe { next_nonnull.as_ref() };

            unsafe {
                ptr::write(
                    tmp_node.data.get(),
                    (self.delegate)(
                        self.data.get().as_mut().unwrap_unchecked(),
                        ptr::read(tmp_node.data.get()),
                    ),
                );
                let work_end = __rdtscp(&mut aux);

                tmp_node.completed.store(true, Release);
                tmp_node.wait.store(false, Release);

                let cs = work_end - work_begin;
                tmp_node
                    .panelty
                    .get()
                    .write(cs * (self.num_waiting_threads.load(Relaxed)));

                work_begin = work_end;
            }

            tmp_node = next_node;
            next_ptr = NonNull::new(tmp_node.next.load(Acquire));
        }

        tmp_node.wait.store(false, Release);

        unsafe {
            self.ban(thread_data, current_node.panelty.get().read());
        }

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            let stat = thread_data.combiner_time_stat.get().as_mut().unwrap();

            stat.combine_time.push(end - begin);

            *stat.combine_size.entry(counter).or_default() += 1;
        }

        return unsafe { current_node.data.get().read() };
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_stat(&self) -> Option<&CombinerStatistics> {
        unsafe {
            self.local_node
                .get()
                .map(|local_node| local_node.combiner_time_stat.get().as_ref().unwrap())
        }
    }
}
