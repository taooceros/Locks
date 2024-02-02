use std::hint::black_box;

use crate::benchmark::bencher::Bencher;

pub fn proportional_counter(bencher: &Bencher) {}

pub fn get_job() -> impl Fn(&mut usize, usize) -> usize {
    return |data: &mut usize, delta: usize| -> usize {
        let data = black_box(data);
        let delta = black_box(delta);
        for _ in 0..delta {
            *data += 1;
        }
        *data
    };
}
