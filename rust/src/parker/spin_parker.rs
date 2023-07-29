use std::sync::atomic::Ordering::{Relaxed, Acquire};
use std::hint::spin_loop;
use std::sync::atomic::AtomicU32;
use std::time::Duration;
use crate::parker::Parker;
use crossbeam::utils::Backoff;

#[derive(Default, Debug)]
pub struct SpinParker {
    flag: AtomicU32,
}

impl Parker for SpinParker {
    fn wait(&self) {
        let backoff = Backoff::default();
        loop {
            let flag = self.flag.load(Acquire);

            if flag < 1 {
                backoff.snooze()
            } else {
                spin_loop()
            }

            if flag == 2 {
                break;
            }
        }
    }

    fn wake(&self) {
        self.flag.store(2, Relaxed);
    }

    fn reset(&self) {
        self.flag.store(0, Relaxed);
    }

    fn prewake(&self) {
        self.flag.fetch_add(1, Relaxed);
    }

    fn wait_timeout(&self, duration: Duration) {
        todo!()
    }
}
