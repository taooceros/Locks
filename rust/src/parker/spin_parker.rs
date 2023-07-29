use std::sync::atomic::Ordering::Relaxed;
use std::hint::spin_loop;
use std::sync::atomic::AtomicU32;
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
            let flag = self.flag.load(Relaxed);

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
}
