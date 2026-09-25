# Offline UpScaleDB analysis and reports

| Module | Purpose |
|---|---|
| `analyze_trials.py` | Validate raw trial/build/source provenance, retain failures, write per-case summaries and optional plots. |
| `campaign_overview.py` | Plot saved fixed-work matrix summaries without rerunning trials. |
| `scaling.py` | Aggregate saved fast scaling cases and placement contrasts. |
| `dimensions.py` | Render the saved joined scaling study by dimension. |
| `hypotheses.py` | Render saved H1/H2/H3 cohorts into a portable report. |

From the repository root, run `python3 -m integration.upscaledb.reports.<module> --help`. For example, `python3 -m integration.upscaledb.reports.analyze_trials --input-dir .worktree/upscaledb/output --no-plots` processes existing runs; the other reports accept `--input-root` and an optional/required `--output-dir` according to their help. Existing analysis artifacts retain their actual filenames and historical hashes; renaming source modules does not relabel completed measurements. Reports consume saved evidence and never launch experiments. Plotting needs the analysis Python/matplotlib environment described in `analyze_trials.py`.
