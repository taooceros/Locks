#!/usr/bin/env bash
# Trimmed matrix ("short" tag): 3 trials, 1 s warmup + 2 s measured, pinned.
# Complements the 5 s "abl" sweep that was stopped at fast_path+cached_tid b-lo.
set -euo pipefail
cd "$(dirname "$0")/../../.."
R="python3 docs/evidence/e0b-fast-path-ablation/runner.py"
S="--tag short --trials 3 --warmup 1 --duration 2"
ALL="baseline cached_tid fast_path fast_path+cached_tid fast_path_notime fast_path+cached_tid+stat"
uptime
# (1) headline, 1 worker, all variants
$R run $ALL $S --cells a-tiny,a-1k
# (2) 32 workers, zero non-CS
$R run baseline fast_path+cached_tid $S --cells c-cs1,c-cs1k
# (3) fairness 1:8 at 8 workers
$R run baseline fast_path fast_path+cached_tid fast_path_notime $S --cells d-het --threads 8
# (4) low contention at 8 workers, with hit rate
$R run baseline fast_path+cached_tid fast_path+cached_tid+stat $S --cells b-lo,b-mid --threads 8
uptime
