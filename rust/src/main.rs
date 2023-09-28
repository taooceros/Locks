use std::{
    fmt::Debug,
    fs::{create_dir, remove_dir_all, File},
    iter::repeat,
    num::NonZeroI64,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use clap::Parser;
use command_parser::*;
use benchmark::benchmark;
use csv::Writer;
use itertools::Itertools;
use quanta::Clock;
use strum::{IntoEnumIterator};

use libdlock::{
    dlock::{BenchmarkType, DLock, DLockType},
    guard::DLockGuard,
    parker::{block_parker::BlockParker, spin_parker::SpinParker, Parker},
    rcl::{rcllock::RclLock, rclserver::RclServer},
};


use serde::Serialize;
use serde_with::serde_as;
use serde_with::DurationMilliSeconds;

mod command_parser;
mod benchmark;


fn main() {
    let mut app = App::parse();

    if app.global_opts.cpus.len() != 1 {
        assert_eq!(app.global_opts.cpus.len(), app.global_opts.threads.len());
    }

    if app.global_opts.cpus.len() == 1 {
        app.global_opts.cpus = repeat(app.global_opts.cpus[0])
            .take(app.global_opts.threads.len())
            .collect();
    }

    let output_path = Path::new(app.global_opts.output_path.as_str());

    if output_path.is_dir() {
        // remove the dir
        match remove_dir_all(output_path) {
            Ok(_) => {}
            Err(e) => {
                println!("Error removing output dir: {}", e);
                return;
            }
        }
    }

    match create_dir(output_path) {
        Ok(_) => {}
        Err(e) => {
            println!("Error creating output dir: {}", e);
            return;
        }
    }


    for (ncpu, nthread) in app
        .global_opts
        .cpus
        .into_iter()
        .zip(app.global_opts.threads)
    {
        benchmark(
            ncpu,
            nthread,
            app.global_opts.experiment,
            app.lock_target,
            output_path,
            app.global_opts.waiter,
            app.global_opts.duration,
        )
    }
}
