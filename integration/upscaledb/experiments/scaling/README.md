# Scaling experiments

| File | Role |
|---|---|
| `comparison_matrix.py` | Serial fixed-work/duration/profile case matrix, invoking the shared runner and analyzer. |
| `run_scaling.py` | Topology-derived physical-core, NUMA, and SMT scaling cases with immutable study plans. |
| `clock_probe.cc` | Separate clock calibration probe; not in the benchmark hot path. |

From the repository root, inspect `python3 -m integration.upscaledb.experiments.scaling.run_scaling --help` and `python3 -m integration.upscaledb.experiments.scaling.comparison_matrix --help`. `run_scaling --plan-only` prints planned cases and commands without running binaries; actual trials need verified builds, eligible CPU/NUMA placement and the measurement lock. `comparison_matrix.py` retains the original packed CPUs `0,1,2,3` and split CPUs `0,1,32,33`; those are machine-specific experiment placements, not portable defaults. The scaling study topology is derived from the actual host; inspect its plan before running. Native `clock_probe.cc` is standalone source, not a Python entrypoint.
