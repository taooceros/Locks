use crate::parker::Parker;
use crossbeam::utils::Backoff;
use quanta::Clock;
use std::cell::SyncUnsafeCell;
use std::hint::spin_loop;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::thread::{current, yield_now, Thread};
use std::time::Duration;

#[derive(Default, Debug)]
pub struct SpinParker {
    flag: AtomicU32,
    last_wake: SyncUnsafeCell<Option<Thread>>,
}

const NOTIFIED: u32 = 2;
const PRENOTIFIED: u32 = 1;
const EMPTY: u32 = 0;

const SPINLIMIT: u32 = 20;

impl Parker for SpinParker {
    fn wait(&self) {
        let backoff = Backoff::default();
        loop {
            match self
                .flag
                .compare_exchange(NOTIFIED, EMPTY, Acquire, Relaxed)
            {
                Ok(_) => return,
                Err(EMPTY) => {
                    backoff.snooze();
                }
                Err(PRENOTIFIED) => {
                    let mut limit = SPINLIMIT - 1;
                    while limit > 0 {
                        if self
                            .flag
                            .compare_exchange(NOTIFIED, EMPTY, Acquire, Relaxed)
                            .is_ok()
                        {
                            return;
                        }
                        spin_loop();
                        limit -= 1;
                    }
                    yield_now();
                }
                Err(value) => panic!("unexpected flag value {}", value),
            }
        }
    }

    fn wake(&self) {
        unsafe {
            *self.last_wake.get() = Some(current());
        }
        self.flag.store(NOTIFIED, Release);
    }

    fn reset(&self) {
        self.flag.store(EMPTY, Relaxed);
    }

    fn prewake(&self) {
        let _ = self
            .flag
            .compare_exchange(EMPTY, PRENOTIFIED, Relaxed, Relaxed);
    }

    fn wait_timeout(&self, timeout: Duration) {
        let clock = Clock::new();
        let begin = clock.now();
        let backoff = Backoff::new();

        loop {
            match self.flag.load(Acquire) {
                NOTIFIED => return,
                PRENOTIFIED => {
                    for _ in 0..SPINLIMIT {
                        if self.flag.load(Acquire) == NOTIFIED {
                            return;
                        }
                        spin_loop();
                    }
                    if clock.now().duration_since(begin) >= timeout {
                        return;
                    }
                    yield_now();
                }
                _ => {
                    if clock.now().duration_since(begin) >= timeout {
                        return;
                    }
                    backoff.snooze();
                }
            }
        }
    }
}
