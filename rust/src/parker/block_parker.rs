use std::{sync::atomic::Ordering::*, hint::spin_loop};
use crossbeam::utils::Backoff;
use linux_futex::{Futex, Private};

use super::Parker;

#[derive(Default, Debug)]
pub struct BlockParker {
    flag: Futex<Private>,
}

impl Parker for BlockParker {
    fn wait(&self) {
        self.flag.value.fetch_add(1, Release);

        self.flag.wait(1).unwrap_or_default();
        let backoff = Backoff::new();
        loop {
            let state = self.flag.value.load(Acquire);

            if state >= 3 {
                break;
            }
            if state == 0 {
                panic!("reset before wake");
            }

            backoff.snooze();
        }
    }

    fn wake(&self) {
        if self.flag.value.swap(3, Release) == 1 {
            self.flag.wake(1);
        }
    }

    fn reset(&self) {
        self.flag.value.store(0, Release);
    }

    fn prewake(&self) {
        if self.flag.value.fetch_add(1, Release) == 1 {
            self.flag.wake(1);
        }
    }
}