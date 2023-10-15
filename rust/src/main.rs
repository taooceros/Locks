use std::{
    fs::{create_dir, remove_dir_all},
    iter::repeat,
    path::Path,
};

use benchmark::benchmark;
use clap::Parser;
use command_parser::*;

mod benchmark;
mod command_parser;

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
        .iter()
        .zip(&app.global_opts.threads)
    {
        benchmark(*ncpu, *nthread, app.lock_target, &app.global_opts)
    }
}
