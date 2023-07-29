use crossbeam::{atomic::AtomicConsume, utils::Backoff};
use linux_futex::{Futex, Private, WaitError::Interrupted};
use std::{
    hint::spin_loop,
    sync::atomic::Ordering::*,
    thread::{current, park_timeout},
    time::Duration,
};

use super::Parker;

const PARKED: u32 = u32::MAX;
const EMPTY: u32 = 0;
const NOTIFIED: u32 = 1;
const PRENOTIFIED: u32 = 2;

#[derive(Default, Debug)]
pub struct BlockParker {
    state: Futex<Private>,
}

impl Parker for BlockParker {
    fn wait(&self) {
        if self.state.value.fetch_sub(1, Acquire) == NOTIFIED {
            return;
        }

        loop {
            // Wait for something to happen, assuming it's still set to PARKED.
            if let Err(Interrupted) = self.state.wait(PARKED) {
                // interruptted wait
                continue;
            }
            // Change NOTIFIED=>EMPTY and return in that case.
            match self
                .state
                .value
                .compare_exchange(NOTIFIED, EMPTY, Acquire, Acquire)
            {
                Ok(_) => {
                    return;
                }
                Err(value) => {
                    if value == PRENOTIFIED {
                        // Change PRENOTIFIED=>PARKED and wait again.
                        let backoff = Backoff::default();

                        while self.state.value.load(Relaxed) == PRENOTIFIED {
                            backoff.snooze();
                        }
                    } else {
                        // Change EMPTY=>PARKED and wait again.
                        continue;
                    }
                }
            }
        }
    }

    fn wait_timeout(&self, timeout: Duration) {
        loop {
            // Change NOTIFIED=>EMPTY or EMPTY=>PARKED, and directly return in the
            // first case.
            if self.state.value.fetch_sub(1, Acquire) == NOTIFIED {
                return;
            }
            // Wait for something to happen, assuming it's still set to PARKED.
            match self.state.wait_for(PARKED, timeout) {
                Ok(_) => todo!(),
                Err(reason) => match reason {
                    linux_futex::TimedWaitError::WrongValue => break,
                    linux_futex::TimedWaitError::Interrupted => continue,
                    linux_futex::TimedWaitError::TimedOut => break,
                },
            }
        }

        // This is not just a store, because we need to establish a
        // release-acquire ordering with unpark().
        let old_value = self.state.value.swap(EMPTY, Acquire);

        if old_value == NOTIFIED {
            // Woke up because of unpark().
        } else if old_value == PRENOTIFIED {
            let backoff = Backoff::default();

            while self.state.value.load(Relaxed) == PRENOTIFIED {
                backoff.snooze();
            }
        } else {
            // Timeout or spurious wake up.
            // We return either way, because we can't easily tell if it was the
            // timeout or not.
        }
    }

    fn wake(&self) {
        if self.state.value.swap(NOTIFIED, Release) == PARKED {
            self.state.wake(1);
        }
    }

    fn reset(&self) {
        self.state.value.store(EMPTY, Release);
    }

    fn prewake(&self) {
        if self
            .state
            .value
            .compare_exchange_weak(PARKED, PRENOTIFIED, Relaxed, Relaxed)
            .is_ok()
        {
            self.state.wake(1);
        }
    }
}
