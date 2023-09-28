use crate::parker::Parker;
use crossbeam::utils::Backoff;
use quanta::Clock;
use std::cell::SyncUnsafeCell;
use std::hint::spin_loop;

use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicU32};
use std::thread::{current, yield_now, Thread};
use std::time::Duration;

use super::State;

#[derive(Default, Debug)]
pub struct SpinParker {
    state: AtomicU32
}

const PARKED: u32 = u32::MAX;
const EMPTY: u32 = 0;
const NOTIFIED: u32 = 1;
const PRENOTIFIED: u32 = 2;
const SPINLIMIT: u32 = 20;

impl Parker for SpinParker {
    fn wait(&self) {
        // change from EMPTY=>PARKED
        // it should not equal to PARKED as this is supposed to be called by one thread
        let backoff = Backoff::default();
        match self.state.compare_exchange(EMPTY, PARKED, Acquire, Relaxed) {
            Ok(_) | Err(PARKED) => loop {
                match self.state.load(Acquire) {
                    NOTIFIED => return,
                    PARKED => {
                        backoff.snooze();
                    }
                    PRENOTIFIED => {
                        self.wait_prenotified();
                    }
                    value => panic!("unexpected flag value {}", value),
                }
            },
            Err(NOTIFIED) => return,
            Err(PRENOTIFIED) => self.wait_prenotified(),
            Err(value) => panic!("unexpected flag value {}", value),
        };
    }

    fn wake(&self) {
        self.state.store(NOTIFIED, Release);
    }

    fn reset(&self) {
        self.state.store(EMPTY, Relaxed);
    }

    fn prewake(&self) {
        let _ = self
            .state
            .compare_exchange(EMPTY, PRENOTIFIED, Relaxed, Relaxed);
    }

    fn wait_timeout(&self, timeout: Duration) -> Result<(), ()> {
        let clock = Clock::new();
        let begin = clock.now();
        let backoff = Backoff::new();

        match self.state.compare_exchange(EMPTY, PARKED, Acquire, Relaxed) {
            Ok(_) | Err(PARKED) => loop {
                match self.state.load(Acquire) {
                    NOTIFIED => return Ok(()),
                    PRENOTIFIED => {
                        for _ in 0..SPINLIMIT {
                            if self.state.load(Acquire) == NOTIFIED {
                                return Ok(());
                            }
                            spin_loop();
                        }
                        if clock.now().duration_since(begin) >= timeout {
                            return Err(());
                        }
                        yield_now();
                    }
                    _ => {
                        if clock.now().duration_since(begin) >= timeout {
                            return Err(());
                        }
                        backoff.snooze();
                    }
                }
            },
            Err(NOTIFIED) => return Ok(()),
            Err(PRENOTIFIED) => {
                self.wait_prenotified();
                return Ok(());
            }
            Err(value) => panic!("unexpected flag value {}", value),
        }
    }

    fn state(&self) -> State {
        return match self.state.load(Acquire) {
            NOTIFIED => State::Notified,
            EMPTY => State::Empty,
            PRENOTIFIED => State::Prenotified,
            PARKED => State::Parked,
            value => panic!("unexpected flag value {}", value),
        };
    }

    fn name() -> &'static str {
        "Spin Parker"
    }
}

impl SpinParker {
    fn wait_prenotified(&self) {
        loop {
            let mut limit = SPINLIMIT - 1;
            while limit > 0 {
                if self.state.load(Acquire) == NOTIFIED {
                    return;
                }
                spin_loop();
                limit -= 1;
            }
            yield_now();
        }
    }
}
