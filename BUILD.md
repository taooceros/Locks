# Build & Test Reference

Requires nightly Rust, GCC/Clang, x86_64. Always use `--release` (debug builds are extremely slow for concurrent code).

## Workspace Layout

Cargo workspace at repo root with two members:
- `.` — binary crate `dlock` (CLI, benchmarks)
- `crates/libdlock` — library crate `libdlock` (lock implementations, traits, tests)

## Commands

```bash
# Build everything
cargo build --release

# Test library locks
cargo test -p libdlock --release --lib

# Test specific module (e.g. dlock2 unit tests only)
cargo test -p libdlock --release --lib dlock2_unit_test

# Test a single lock variant
cargo test -p libdlock --release --lib dlock2_unit_test::shfl_lock

# Force rebuild C code (cargo doesn't detect C source changes reliably)
cargo clean -p libdlock && cargo test -p libdlock --release --lib

# Run benchmarks
cargo run --release -- d-lock2 counter-proportional --cs 1000,3000 --non-cs 0
cargo run --release -- d-lock2 counter-proportional --lock-targets fc,shfl-lock -t 4,8

# Justfile shortcuts (from repo root)
just build
just run2                        # all locks, default CS
just run2 "fc,shfl-lock"         # specific locks
```

## Gotchas

- **C code rebuild**: `cargo` does not always detect changes to C source files under `c/`. If you modify C code and tests still fail with old behavior, run `cargo clean -p libdlock` to force a full rebuild.
- **Debug builds**: Never benchmark or test concurrent code in debug mode — it is orders of magnitude slower and can cause spurious timeouts.
