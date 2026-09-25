# UpScaleDB: start here

For a normal comparison, use four tools in order:

| Step | Module | What it does |
|---|---|---|
| Build | `integration.upscaledb.core.build` | Build pinned upstream/control/lock variants |
| Check | `integration.upscaledb.core.correctness` | Check real DB results, lifecycle and error behavior |
| Run | `integration.upscaledb.runner.run_trials` | Execute fresh processes and retain every result/failure |
| Analyze | `integration.upscaledb.reports.analyze` | Validate and summarize saved results; no experiments launched |

This uses the existing runner, not a new wrapper or framework. Optional specialized
studies live under [experiments/](experiments/README.md). See [reports/](reports/README.md)
for multi-case reports and [tests/](tests/README.md) for source-level checks.

## Minimal workflow

First enter the environment and set `MEASUREMENT_LOCK` as described in
[the shared guide](../README.md). Run all commands from the repository root.
The example builds five primary variants and the separate correctness-test binary
in the default `.worktree/upscaledb` directory:

```sh
export VARIANTS=native,refactored,bridge_mutex,fc,fc_pq
flock --shared "$MEASUREMENT_LOCK" \
  python3 -m integration.upscaledb.core.build \
  --variants "$VARIANTS,test_hooks" --jobs 8

flock --exclusive "$MEASUREMENT_LOCK" prlimit --core=0 \
  python3 -m integration.upscaledb.core.correctness \
  --bin-dir .worktree/upscaledb \
  --variants native refactored bridge_mutex fc fc_pq
```

Build into a fresh directory initially. Existing artifacts require explicit
`--resume` and matching provenance. Manifests include source locations: builds from
the old flat layout do not automatically qualify for reuse. Keep those artifacts
intact and use a fresh root.
See [core/](core/README.md) for alternate build roots and other variants.
The correctness gate uses 128 workers: do not restrict its process to the eight
CPUs selected for the smaller workload below. Test-hook children intentionally abort.

Choose eight eligible, distinct physical cores on one NUMA node, replacing these
example IDs. Run a small, explicitly labeled smoke comparison:

```sh
export CPUS=16,17,18,19,20,21,22,23
export NUMA_NODE=0
export RUN=.worktree/upscaledb/runs/smoke-01

flock --exclusive "$MEASUREMENT_LOCK" taskset -c "$CPUS" \
  python3 -m integration.upscaledb.runner.run_trials \
  --variants "$VARIANTS" --mode fixed --roles 4+4 \
  --cpus "$CPUS" --layout packed --exclusive-cpus \
  --memory-policy bind --memory-nodes "$NUMA_NODE" \
  --repetitions 1 --preload 64 --reads 128 --inserts 128 \
  --max-inserts 1024 --memory-limit-gib 32 \
  --warmup 0 --smoke --output-dir "$RUN"

flock --shared "$MEASUREMENT_LOCK" \
  python3 -m integration.upscaledb.reports.analyze --input-dir "$RUN"
```

`--layout packed` labels the chosen CPU placement; it does not select CPUs for you.
`--smoke` labels the run and checks small bounds; it does not silently reduce counts.
For a primary study, choose a new `RUN`, omit `--smoke`, and explicitly choose the
repetition count/work size. For sustained-load studies use `--mode duration` and
`--seconds`; do not compare their throughput ratios as fixed-work speedups.
Use separately labeled profile variants/cohorts when measuring service allocation.

### Where are the results?

- `$RUN/manifest.json`: configuration, randomized order, commands and provenance.
- `$RUN/block-*.json`: raw outcomes, including failures.
- `$RUN/analysis/summary.json` and `summary.csv`: validated summaries.
- `$RUN/analysis/failures.csv`: invalid, failed or missing trials.

Check `failure_count` in `summary.json` before interpreting estimates. Keep failure
records; never selectively rerun cells to obtain a favorable result. One smoke
repetition provides no performance evidence.

## What belongs in each folder?

| Folder | Needed for the standard workflow? |
|---|---|
| [core/](core/README.md) | Yes: builder, adapter/patches, native harness and DB correctness gate |
| [runner/](runner/README.md) | Yes: standard execution and source/result capture |
| [reports/](reports/README.md) | `analyze` for one cohort; other reports are optional |
| [experiments/](experiments/README.md) | Optional: predefined scaling, hypothesis and boundary studies |
| [tests/](tests/README.md) | Development checks for provenance, placement and runner behavior |

`_paths.py` only centralizes repository/folder locations. It does not run an
experiment or define a new configuration framework.

## Application-logic boundary

Each submission remains one original find or insert. The adapter extracts the
complete existing environment-mutex critical-section body into a shared helper;
the public/refactored control and delegated path use that same body. Native uses
the original upstream implementation. FC/FC-PQ may execute a request on a combiner;
other lock backends execute it on the requesting worker. That synchronization
change is the treatment—not a rewritten database operation.

Controls are **Native**, **refactored with the original mutex**, and **bridge using
the borrowed original mutex**. The adapter is restricted to synchronous in-memory,
nontransactional, fixed-width key/record operations with caller-owned output. It is
not an unmodified UpScaleDB binary or a transparent replacement of the entire API.
Unsupported schemas/flags are not approximated; callers join before exclusive
destruction, and exceptions/reentry/unwinding cannot cross the Rust/C ABI.

No batch8 extension, split-environment/multitable restructuring or dashboard is
included. Historical experiment worktrees/results are untouched. CFL-local is a
local adaptation, not a verified author artifact; its historical 128-worker gate
timed out when limited to eight CPUs. CLH retains raw queue nodes until process
exit. ShflLock is excluded. These limits are not erased by reorganizing the files.
