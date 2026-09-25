# UpScaleDB experiment controllers

Run modules from the repository root with `python3 -m integration.upscaledb.experiments.<group>.<module>`. These are research controllers, not application code; the shared single-operation runner and native build live in [`../runner/`](../runner/) and [`../core/`](../core/). Prepare only against verified frozen builds, retain failed output roots, and keep timed runs under the measurement lock described in [`../../README.md`](../../README.md).

| Folder | Purpose |
|---|---|
| [`scaling/`](scaling/) | Fixed-work/duration matrix, physical-core/NUMA/SMT scaling, and independent clock probe. |
| [`hypotheses/`](hypotheses/) | Finder/inserter role asymmetry, synthetic reservation diagnostic, intermittent arrivals, and cohort placement. |
| [`boundaries/`](boundaries/) | Controlled synthetic callback cost, Poisson arrival, and single-database working-set boundaries. |

CPU IDs, memory nodes, and output roots are experiment-specific; read each group before scheduling on another machine. Neither this layout nor the module commands change the workload or establish new measurements.
