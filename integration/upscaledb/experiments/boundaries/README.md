# Boundary studies

| File | Role |
|---|---|
| `boundary_cost.py`, `.cc` | Synthetic fixed callback-cost asymmetry using verified frozen bridge libraries; not database throughput. |
| `boundary_arrival.py`, `.cc` | Synthetic independent Poisson arrivals to the original restricted real-database operations. |
| `boundary_database.py` | Closed-loop single-database preload/worker-count study using frozen binaries. |

From the repository root: `python3 -m integration.upscaledb.experiments.boundaries.<module> --help`. Each controller separates `--prepare-only`, `--run`, and `--analyze-only` with an explicit `--output-root`. `boundary_cost` originally requires CPUs 20–27/node 0, `boundary_arrival` CPUs 40–47/node 1, and `boundary_database` CPUs 48–55/node 1; preparation checks the requested machine placement rather than remapping it silently. Use the specified external measurement/slot locks, verify frozen source and binary hashes, and do not overwrite failed runs. The `.cc` files are compiled during preparation, not standalone Python commands.
