# Hypothesis studies

| File | Role |
|---|---|
| `hypothesis_cohort.py` | Record/check disjoint CPU/NUMA placements for simultaneous explorations. |
| `hypothesis_roles.py` | H1 real-database finder/inserter role and service asymmetry. |
| `hypothesis_reservation.py`, `.cc` | H2 synthetic two-actor reserved-slice diagnostic, **not** database throughput. |
| `hypothesis_bursts.py`, `.cc` | H3 intermittent-client wrapper reusing the single-operation native harness. |

Run from the repository root with `python3 -m integration.upscaledb.experiments.hypotheses.<module> --help`; for H1/H2/H3, use separate `--prepare-only`, `--run`, and `--analyze-only` stages with `--output-root` and verified frozen build inputs. Create the resource record with `python3 -m integration.upscaledb.experiments.hypotheses.hypothesis_cohort --output-root ROOT` before coordinated runs. The original simultaneous placement assumes H1 CPUs 0–7/node 0, H2 CPUs 16–17/node 0 and H3 CPUs 32–39/node 1; these are site-specific and checked, not portable recommendations. Preserve incomplete/failed roots; the reports read saved data, not reruns.
