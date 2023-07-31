use crossbeam::utils::Backoff;
use linux_futex::{Futex, Private, TimedWaitError, WaitError::Interrupted};
use std::{sync::atomic::Ordering::*, time::Duration};

use super::Parker;

const PARKED: u32 = u32::MAX;
const EMPTY: u32 = 0;
const NOTIFIED: u32 = 1;
const PRENOTIFIED: u32 = PARKED - 1;

#[derive(Default, Debug)]
pub struct BlockParker {
    state: Futex<Private>,
}

impl Parker for BlockParker {
    fn wait(&self) {
        match self
            .state
            .value
            .compare_exchange(EMPTY, PARKED, Acquire, Acquire)
        {
            Err(NOTIFIED) => return,
            Err(PRENOTIFIED) => self.wait_prenotified(),
            Ok(_) | Err(PARKED) => loop {
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
            },
            value => panic!("unexpected value: {:?}", value),
        }
    }

    fn wait_timeout(&self, timeout: Duration) -> Result<(), ()> {
        loop {
            // Change NOTIFIED=>EMPTY or EMPTY=>PARKED, or PRENOTIFIED=>PRENOTIFIED2, and directly return in the
            // first case.
            match self
                .state
                .value
                .compare_exchange(EMPTY, PARKED, Acquire, Acquire)
            {
                Err(NOTIFIED) => return Ok(()),
                Err(PRENOTIFIED) => {
                    self.wait_prenotified();
                    return Ok(());
                }
                Ok(_) | Err(PARKED) => {
                    // Wait for something to happen, assuming it's still set to PARKED.
                    match self.state.wait_for(PARKED, timeout) {
                        Ok(_) => return Ok(()),
                        Err(reason) => match reason {
                            TimedWaitError::WrongValue => {}
                            TimedWaitError::TimedOut => return Err(()),
                            TimedWaitError::Interrupted => continue,
                        },
                    };

                    let state = self.state.value.load(Acquire);

                    match state {
                        NOTIFIED => return Ok(()),
                        PRENOTIFIED => {
                            self.wait_prenotified();
                            return Ok(());
                        }
                        _ => panic!("unexpected state: {:?}", state),
                    }
                }
                value => panic!("unexpected value: {:?}", value),
            }
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
        let mut old_value = self.state.value.load(Relaxed);

        loop {
            if old_value == NOTIFIED {
                return;
            }

            match self
                .state
                .value
                .compare_exchange_weak(old_value, PRENOTIFIED, Relaxed, Relaxed)
            {
                Ok(EMPTY) => return,
                Ok(PARKED) => {
                    self.state.wake(1);
                    return;
                }
                Err(value) => old_value = value,
                value => panic!("unexpected value: {:?}", value),
            };
        }
    }

    fn state(&self) -> super::State {
        return match self.state.value.load(Acquire) {
            NOTIFIED => super::State::Notified,
            EMPTY => super::State::Empty,
            PARKED => super::State::Parked,
            PRENOTIFIED => super::State::Prenotified,
            _ => panic!(),
        };
    }
}

impl BlockParker {
    #[inline]
    fn wait_prenotified(&self) {
        let backoff = Backoff::default();

        // theoredically it will not equal to PRENOTIFIED2
        while matches!(self.state.value.load(Relaxed), PRENOTIFIED) {
            backoff.snooze();
        }
    }
}
