# Boundary studies

| File | Role |
|---|---|
| `operation_cost.py`, `.cc` | Synthetic fixed callback-cost asymmetry using verified frozen bridge libraries; not database throughput. |
| `arrival_rate.py`, `.cc` | Synthetic independent Poisson arrivals to the original restricted real-database operations. |
| `database_load.py` | Closed-loop single-database preload/worker-count study using frozen binaries. |

From the repository root: `python3 -m integration.upscaledb.experiments.boundaries.<module> --help`. Each controller separates `--prepare-only`, `--run`, and `--analyze-only` with an explicit `--output-root`. `operation_cost` requires CPUs 20–27/node 0, `arrival_rate` CPUs 40–47/node 1, and `database_load` CPUs 48–55/node 1; preparation checks the requested machine placement rather than remapping it silently. Use the specified external measurement/slot locks, verify frozen source and binary hashes, and do not overwrite failed runs. The `.cc` files are compiled during preparation, not standalone Python commands.
