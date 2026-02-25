# Locks

Research project implementing usage-fair delegation locks in Rust.

## Binary Crate (`dlock`)

Benchmark harness and CLI for evaluating delegation lock implementations.

### Requirements

- **Nightly Rust** (uses `sync_unsafe_cell`, `type_alias_impl_trait`, `thread_id_value`, etc.)
- **GCC or Clang** (compiles C reference implementations via `build.rs`)
- **x86_64 only** (uses `__rdtscp` for cycle-accurate timing)

### Build & Run

```bash
cargo build --release
cargo test --release --verbose
cargo fmt --check
cargo clippy --release
```

### CLI

```
dlock [GLOBAL_OPTIONS] <EXPERIMENT>

EXPERIMENTS:
    d-lock2   <DLock2Experiment>      # Function-delegate API (primary)
    d-lock1   <DLock1Experiment>      # Callback-based API (legacy)

GLOBAL OPTIONS:
    -t, --threads <N,...>      Thread counts to test [default: CPU count]
    -c, --cpus <N,...>         CPU counts to test [default: CPU count]
    -o, --output-path <path>   [default: visualization/output]
    -d, --duration <secs>      Measurement duration [default: 5]
    --warmup <secs>            Warmup period (no stats) [default: 2]
    --trials <N>               Independent trials [default: 1]
    --stat-response-time       Collect per-op latency CDFs
    -v, --verbose
```

### DLock2 Experiments

- `counter-proportional` — Shared counter with configurable CS length (`--cs`, `--non-cs`)
- `fetch-and-multiply` — Multiply on shared counter
- `queue` — Enqueue/dequeue on shared queue
- `priority-queue` — Insert/extract-min on shared PQ

### DLock2 Lock Targets (`--lock-targets`)

`fc`, `fc-ban`, `cc`, `cc-ban`, `dsm`, `fc-sl`, `fc-pq-b-tree`, `fc-pq-b-heap`, `spin-lock`, `mcs`, `mutex`, `uscl`, `fc-c`, `cc-c`, `shfl-lock`, `aqs-c`

### Examples

```bash
cargo run --release -- d-lock2 counter-proportional --cs 1000,3000 --non-cs 0
cargo run --release -- d-lock2 counter-proportional --lock-targets fc,fc-pq-b-tree --cs 1000 -t 4,8,16
cargo run --release -- --help
```

## Crate Structure

```
src/                            # Binary crate (CLI, benchmarks)
├── main.rs
├── command_parser.rs
├── command_parser/
│   ├── experiment.rs
│   └── lock_target.rs
└── benchmark/                  # See src/benchmark/README.md

crates/libdlock/                # Library crate (lock implementations)
├── src/
│   ├── dlock2/                 # Function-delegate API (primary)
│   ├── dlock/                  # Callback-based API (legacy)
│   └── parker/                 # Thread parking strategies
└── binding/                    # C FFI wrapper headers

c/                              # C reference implementations
visualization/                  # Jupyter notebooks & plots
```

## Output

Arrow IPC `.arrow` files written to `<output_path>/<lock_name>/`. Each file contains per-thread records: loop counts, latencies, hold times, JFI, combiner stats.

## Justfile Shortcuts

```bash
just build                    # cargo build --release
just run2                     # d-lock2 counter-proportional --cs 1000,3000 --non-cs 0
just run2 "fc,fcpq"          # specific lock targets
just run1                     # d-lock1 variant
```
