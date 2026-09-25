# redb transaction experiment

This folder is self-contained apart from its shared `libdlock` dependency. It uses
**unmodified redb 3.1.0**. Native uses redb's single-writer boundary directly;
Mutex/MCS/FC/FC-PQ wrap complete transactions without moving a transaction guard
across threads. The client scheduler is an experimental treatment, not an unchanged
production workload.

| File | Purpose |
|---|---|
| `Cargo.toml`, `Cargo.lock` | Standalone, dependency-pinned Cargo workspace |
| `src/main.rs` | Real transaction workload and exact-content verification |
| `run.py` | Preparation, fresh-process trials, smoke and offline analysis |

## Usage

Enter the environment and set `MEASUREMENT_LOCK` using [the shared guide](../README.md).
Run from the repository root. Choose eight available CPUs and a suitable NUMA node;
the following placement is illustrative:

```sh
export CPUS=16,17,18,19,20,21,22,23
export NUMA_NODE=0
export RUN=.worktree/redb-campaign

flock --shared "$MEASUREMENT_LOCK" taskset -c "$CPUS" \
  python3 -m integration.redb.run --prepare-only \
  --output-root "$RUN" --cpus "$CPUS" --numa-node "$NUMA_NODE"

flock --exclusive "$MEASUREMENT_LOCK" taskset -c "$CPUS" \
  python3 -m integration.redb.run --smoke --output-root "$RUN"
```

Preparation builds primary/profile variants and freezes their source/binary identity,
CPU order and memory policy. Smoke covers twenty real DB cells, separately from
the formal matrix. Reusing a populated preparation/smoke directory is rejected;
use fresh paths rather than overwrite results.

Run the formal workload only when intended, then analyze its complete cohort:

```sh
flock --exclusive "$MEASUREMENT_LOCK" taskset -c "$CPUS" \
  python3 -m integration.redb.run --run --output-root "$RUN"
flock --shared "$MEASUREMENT_LOCK" \
  python3 -m integration.redb.run --analyze-only --output-root "$RUN"
```

The formal matrix contains 180 cells: five backends, three transaction mixes,
two durability modes, three repetitions and primary/profile builds. Eight clients
use all-one-record transactions, or four small/four 8- or 64-record transactions.
Transaction sizes and durability match across backends within each cohort.

Outputs: `manifest.json`, `build/`, `binaries/`, `smoke/`, `runs/` and `analysis/`
under `$RUN`. The runner checks contents/close-reopen, records database hashes and
removes database files while retaining raw outcomes. Analysis requires the formal
matrix; smoke alone is not enough.

Use `--numa-node none` at preparation only when intentionally omitting memory binding
and report that qualification. `--target-root` defaults to `.worktree/redb-cargo-target`;
`--build-jobs` controls compilation concurrency. Capacity guards remain 8 million
records per worker and a 512 MiB database-file limit, not silent truncation.

Immediate and None durability are separate experiments. Close/reopen is not a
power-loss test; None is not a durable-commit promise. Primary/profile results remain
separate because instrumentation can change scheduling. See `--help` for all options.
