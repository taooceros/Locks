# UpScaleDB trial runner

`run_trials.py` runs each trial in a fresh process using already built binaries; it records raw trial JSON, the machine/configuration manifest, source archives, and build provenance. It does not build variants or choose a CPU placement for you.

Start with the [standard build → check → run → analyze workflow](../README.md#minimal-workflow),
which includes placement and measurement-lock commands. For the full runner options:

```sh
python3 -m integration.upscaledb.runner.run_trials --help
```

The default output directory is `.worktree/upscaledb/output/`, which must be empty; use `--output-dir` for a fresh run. Each binary needs its matching `build-VARIANT.json` (use `--binary VARIANT=PATH` with `--build-manifest VARIANT=PATH` for external builds). Defaults remain 10 repetitions, 4+4 workers, and 120 seconds per trial; `--smoke` labels a small run but does not shrink its parameters. Supply workload, topology, and memory-policy options explicitly when comparing results.
