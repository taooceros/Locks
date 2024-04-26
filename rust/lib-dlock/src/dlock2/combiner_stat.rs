use std::collections::HashMap;
use std::ops::{AddAssign, SubAssign};
use std::{arch::x86_64::__rdtscp, usize};

#[derive(Debug, Default)]
pub struct CombinerSample {
    pub combine_size: HashMap<usize, usize>,
    pub combine_time: Vec<u64>,
}
static mut last_count: usize = usize::MAX;
static mut last_combiner: Option<std::thread::ThreadId> = None;

impl CombinerSample {
    #[cfg(feature = "combiner_stat")]
    pub(crate) fn insert_sample(&mut self, begin: u64, count: usize) {
        unsafe {
            let mut aux: u32 = 0;
            let end = __rdtscp(&mut aux);
            self.combine_time.push(end - begin);
            if let Some(thread) = last_combiner {
                if thread == std::thread::current().id() && last_count != usize::MAX {
                    self.combine_size
                        .entry(last_count)
                        .or_default()
                        .sub_assign(1);

                    self.combine_size
                        .entry(last_count + count)
                        .or_default()
                        .add_assign(1);

                    last_count += count;
                    return;
                }
            }
            self.combine_size.entry(count).or_default().add_assign(1);

            last_count = count;
            last_combiner = Some(std::thread::current().id());
        }
    }
}
