# UpScaleDB trial runner

`run_trials.py` runs each trial in a fresh process using already built binaries; it records raw trial JSON, the machine/configuration manifest, source archives, and build provenance. It does not build variants or choose a CPU placement for you.

Start with the [standard build → check → run → analyze workflow](../README.md#minimal-workflow),
which includes placement and measurement-lock commands. For the full runner options:

```sh
python3 -m integration.upscaledb.runner.run_trials --help
```

The default output directory is `.worktree/upscaledb/output/`, which must be empty; use `--output-dir` for a fresh run. Each binary needs its matching `build-VARIANT.json` (use `--binary VARIANT=PATH` with `--build-manifest VARIANT=PATH` for external builds). Defaults remain 10 repetitions, 4+4 workers, and 120 seconds per trial; `--smoke` labels a small run but does not shrink its parameters. Supply workload, topology, and memory-policy options explicitly when comparing results.

## Shared process mechanics

`process_execution.py` contains two functions used by the specialized controllers:

- `capture_command`: captured stdout/stderr, a hard process-group timeout, and a
  retained outcome even on nonzero exit or launch failure.
- `run_logged`: an exclusive combined log and atomic event record; SIGTERM allows
  a nested runner to clean up, followed by SIGKILL if its explicit grace period expires.

Controllers retain their command construction, schedules, validators, source
inventories and context fields. Role-balance and database-load studies keep their
20-second and 30-second grace periods respectively. Cost/reservation probes retain
immediate timeout kill and a five-second pipe drain.

This is not a universal runner framework. `run_trials.py` keeps its specialized
signal deferral, binary identity checks and byte-preserving cleanup. redb keeps its
file-size limits and result handling. Combining those policies would require more
options and obscure differences rather than simplify them.

Prepared studies hash their controller and helper sources. After this refactor,
prepare a fresh study root; do not rewrite an old manifest to make it pass.
Native builds need not be repeated when their own source/binary provenance still matches.
