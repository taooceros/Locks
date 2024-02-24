extern crate bindgen;

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // This is the directory where the `c` library is located.
    let libdir_path = PathBuf::from("../../c/build/linux/x86_64/release")
        // Canonicalize the path as `rustc-link-search` requires an absolute
        // path.
        .canonicalize()
        .expect("cannot canonicalize path");

    // This is the path to the `c` headers file.
    let headers_path = PathBuf::from("binding")
        .canonicalize()
        .expect("cannot canonicalize path")
        .join("wrapper.h");
    let headers_path_str = headers_path.to_str().expect("Path is not a valid string");

    // This is the path to the static library file.
    let lib_path = libdir_path.join("libdlocks.a");

    let c_code_path = PathBuf::from("../../c");

    // xmake build

    Command::new("xmake")
        .current_dir(c_code_path)
        .status()
        .expect("failed to build c code");

    // Tell cargo to look for shared libraries in the specified directory
    println!("cargo:rustc-link-search={}", libdir_path.to_str().unwrap());

    // Tell cargo to tell rustc to link our `dlocks` library. Cargo will
    // automatically know it must look for a `libdlocks.a` file.
    println!("cargo:rustc-link-lib=dlocks");

    // Tell cargo to invalidate the built crate whenever the header changes.
    println!("cargo:rerun-if-changed={}", headers_path_str);

    // The bindgen::Builder is the main entry point
    // to bindgen, and lets you build up options for
    // the resulting bindings.
    let bindings = bindgen::Builder::default()
        // The input header we would like to generate
        // bindings for.
        .header(headers_path_str)
        .clang_arg("-I../../c/shared")
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .blocklist_function("qgcvt")
        .blocklist_function("qgcvt_r")
        .blocklist_function("qfcvt")
        .blocklist_function("qfcvt_r")
        .blocklist_function("qecvt")
        .blocklist_function("qecvt_r")
        .blocklist_function("strtold")
        .blocklist_function("strtof64x_l")
        .blocklist_function("strtold_l")
        .blocklist_function("strfroml")
        .blocklist_function("strfromf64x")
        .blocklist_function("strtof64x")
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings
        .write_to_file(out_path)
        .expect("Couldn't write bindings!");
}
