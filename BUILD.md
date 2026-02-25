# Build & Test Reference

All Rust code lives under `rust/`. Requires nightly Rust, GCC/Clang, x86_64. Always use `--release` (debug builds are extremely slow for concurrent code).

## Workspace Layout

`libdlock` is a path dependency of `dlock`, **not** a workspace member. You cannot use `-p libdlock` from the workspace root. Run library tests from `rust/lib-dlock/` directly.

## Commands

```bash
# Build (from rust/)
cd rust && cargo build --release

# Test library locks — must run from lib-dlock/, not workspace root
cd rust/lib-dlock && cargo test --release --lib

# Test specific module (e.g. dlock2 unit tests only)
cd rust/lib-dlock && cargo test --release --lib dlock2_unit_test

# Test a single lock variant
cd rust/lib-dlock && cargo test --release --lib dlock2_unit_test::shfl_lock

# Force rebuild C code (cargo doesn't detect C source changes reliably)
cd rust/lib-dlock && cargo clean && cargo test --release --lib

# Run benchmarks (from rust/)
cd rust && cargo run --release -- d-lock2 counter-proportional --cs 1000,3000 --non-cs 0
cd rust && cargo run --release -- d-lock2 counter-proportional --lock-targets fc,shfl-lock -t 4,8

# Justfile shortcuts (from repo root)
just build
just run2                        # all locks, default CS
just run2 "fc,shfl-lock"         # specific locks
```

## Gotchas

- **C code rebuild**: `cargo` does not always detect changes to C source files under `c/`. If you modify C code and tests still fail with old behavior, run `cargo clean` from `rust/lib-dlock/` to force a full rebuild.
- **Debug builds**: Never benchmark or test concurrent code in debug mode — it is orders of magnitude slower and can cause spurious timeouts.
- **`-p libdlock` doesn't work**: Since `libdlock` is not a workspace member, `cargo test -p libdlock` from `rust/` will fail. Always `cd rust/lib-dlock` first.
