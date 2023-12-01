use std::{
    fs::{self, remove_dir_all, DirBuilder, Permissions},
    iter::repeat,
    os::unix::{fs::DirBuilderExt, prelude::PermissionsExt},
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
        remove_dir_all(output_path).expect("Failed to remove output dir");
    }

    DirBuilder::new()
        .mode(0o777)
        .create(output_path)
        .expect("Failed to create output dir");

    fs::set_permissions(output_path, Permissions::from_mode(0o777)).unwrap();


    for (ncpu, nthread) in app.global_opts.cpus.iter().zip(&app.global_opts.threads) {
        benchmark(*ncpu, *nthread, app.lock_target, &app.global_opts)
    }
}
