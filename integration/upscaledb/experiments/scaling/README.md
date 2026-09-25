# Scaling experiments

| File | Role |
|---|---|
| `evaluate.py` | Serial fixed-work/duration/profile case matrix, invoking the shared runner and analyzer. |
| `scaling.py` | Topology-derived physical-core, NUMA, and SMT scaling cases with immutable study plans. |
| `clock_probe.cc` | Separate clock calibration probe; not in the benchmark hot path. |

From the repository root, inspect `python3 -m integration.upscaledb.experiments.scaling.scaling --help` and `python3 -m integration.upscaledb.experiments.scaling.evaluate --help`. `scaling --plan-only` prints planned cases and commands without running binaries; actual trials need verified builds, eligible CPU/NUMA placement and the measurement lock. `evaluate.py` retains the original packed CPUs `0,1,2,3` and split CPUs `0,1,32,33`; those are machine-specific experiment placements, not portable defaults. The scaling study topology is derived from the actual host; inspect its plan before running. Native `clock_probe.cc` is standalone source, not a Python entrypoint.
