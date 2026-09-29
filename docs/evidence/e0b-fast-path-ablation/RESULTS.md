# E0(b) FC-PQ low-contention fast path: ablation results

Plan: [plan/2026-09-27/e0b-fcpq-fast-path.md](../../../plan/2026-09-27/e0b-fcpq-fast-path.md).
Workspace `e0b-fastpath` (jj change `ouppsvtn`). Every binary was built from source digest
`d298bdb0890ce3fb` (sha256 of `crates/libdlock/src/**/*.rs`, `src/**/*.rs` and both Cargo.toml files). That
build came after FastPathImpl's 23:35 double-execution fix; build records are in
`raw/builds/*.json` (see §11 for where raw data lives). The unpinned `cpu0-*` noise runs used an earlier no-feature
build (digest `2383697978f51248`), and they were used only as a noise check.

The matrix was trimmed on Main's instruction ("finish faster"). The full 5 s sweep
(`abl`) was stopped during `fast_path+cached_tid` b-lo 4W. Cells that decide the
recommendation were then filled with a short protocol (`short`). Missing cells are
listed in §6.

## 1. Method

- Workload: `dlock d-lock2 counter-proportional`. The CS is `--cs N` increments of a
  shared counter, and there are `--non-cs M` empty loop iterations between requests. A
  comma list is assigned to threads round-robin. Every trial runs FC (`fc`, the
  non-fair reference) and FC-PQ (`fc-pq-b-heap`) back to back in the same process,
  and ratios pair them by trial.
- Cells: a-tiny = 1W, cs 1. a-1k = 1W, cs 1000 (≈530 ns of CS). b-lo = cs 100,
  non-cs 100 000. b-mid = cs 100, non-cs 10 000. c-cs1 / c-cs1k = 32W, cs 1 / 1000,
  non-cs 0. d-het = cs 1000,8000 (1:8), non-cs 0.
- Pinning: `taskset` restricts the process to node-1 physical cores, and the harness
  pins worker i to the i-th CPU of that mask. 1W uses CPU 48; multi-worker cells use
  32..32+T-1. The unpinned `cpu0-*` runs used CPUs 0..T-1.
- Protocols:
  - `abl`: 2 s warmup + 5 s measured, with 10 trials (1W) or 5 trials (others).
  - `short`: 1 s warmup + 2 s measured, 3 trials.
  - `cpu0-r1/r2`: same as `abl`, unpinned, baseline only.
  - Tables give the median and [min, max] over trials.
- Jain = the harness's JFI over per-thread CS hold time (TSC ticks inside the
  delegate), i.e. a service-time Jain index.
- Hit rate = `FCPQ::fast_path_hits()` / all `lock()` calls, both including warmup.
  It needs the `fcpq_fast_path_stat` build and is printed by `proportional_counter.rs`.
- **Net overhead** = ns/op − measured CS hold per op, with TSC = 2200.2 MHz from
  `/home/hongtao/Locks/.worktree/tsc-accuracy/RESULTS.md`. This metric is needed
  because the same `--cs 1000` loop costs 507-610 ns depending on the binary. Code
  layout changes with the feature set, and that affects FC too: in the cached_tid
  builds FC is ~10 % slower at a-1k although FC's code is unchanged. The raw paired
  ratio at a-1k (and in other CS-heavy cells) therefore carries up to ±10 % of layout
  noise. At 1W, net overhead is the per-request lock cost. In saturated cells (c, d)
  it is the serial per-op protocol cost.

## 2. Headline: 1 worker (`short`, 3 trials; `abl` 10-trial repeats agree to ≤1.2 ns)

| variant | a-tiny FC-PQ Mops/s | a-tiny paired FC-PQ/FC | FC-PQ net ns/op | tax vs same-binary FC | a-1k paired FC-PQ/FC | a-1k hold-corrected ratio | hit rate |
|---|---|---|---:|---:|---|---:|---|
| FC reference | 13.84-13.94 | — | 62.0-62.7 | — | — | — | — |
| baseline | 7.316 [7.314, 7.318] | **0.528** [0.526, 0.543] | 127.1 | +64.7 | 0.887 [0.883, 0.891] | **0.901** | — |
| cached_tid | 8.065 [8.054, 8.075] | **0.579** [0.577, 0.579] | 114.4 | +52.4 | 0.972 [0.970, 0.986]* | **0.920** | — |
| fast_path | 15.535 [15.511, 15.540] | **1.123** [1.119, 1.123] | 54.6 | −8.1 | 1.016 [1.014, 1.016] | **1.013** | — |
| fast_path+cached_tid | 15.316 [15.315, 15.323] | **1.104** [1.102, 1.110] | 55.8 | −6.5 | 0.941 [0.939, 0.946]* | **1.012** | — |
| fast_path_notime | 23.275 [23.152, 23.276] | **1.678** [1.667, 1.678] | 33.4 | −28.9 | 1.043 [1.034, 1.046] | **1.052** | — |
| fast_path+cached_tid+stat | 15.266 [15.215, 15.287] | **1.097** [1.094, 1.097] | 56.0 | −6.2 | 0.995 [0.977, 1.009]* | **1.011** | 1.000 (both cells) |

\* Layout artifact: in these binaries FC's own CS loop is 45-66 ns slower (hold
574/564/533 ns vs 508-515 ns). The hold-corrected ratio uses each binary's net
overheads with a common CS (the median hold, 528.3 ns), i.e.
(528.3 + FC ovh)/(528.3 + FC-PQ ovh).

The tax is additive and does not depend on CS length: +64.7 ns at cs 1 and +65.7 ns
at cs 1000 for the baseline. This agrees with the 67.5 ns estimate in the
TSC-accuracy study. Plain `fast_path`, `fast_path_notime` and the stat build meet
the ≥ 0.95 target at 1 worker on the raw paired ratio, at both CS lengths.
**`fast_path+cached_tid` meets it at tiny CS (1.104) but misses on the raw paired
a-1k ratio (0.941 [0.939, 0.946]).** It passes only after hold correction (1.012),
because in that binary the FC-PQ CS loop itself runs 610 ns vs 507 ns in the
fast_path binary, while its net lock overhead (61.3 ns) equals fast_path's
(62.1 ns). The baseline's 1W ratio is 0.53 (tiny CS) and 0.90 (cs 1000), a larger
gap than the plan's "+28 %".

## 3. Attribution of the 1-worker tax (a-tiny net overhead, `short`; `abl` agrees to ≤1.2 ns)

Baseline FC-PQ costs 127.1 ns/request, FC costs 62.4 ns, so the tax is 64.7 ns.

- **cached_tid (D6)** removes **12.7 ns**, 20 % of the tax. This is the
  `current().id()` Thread clone+drop on every re-enrollment. Once the fast path is on
  it gives nothing (fast_path+cached_tid is 55.8 ns vs 54.6 ns for fast_path),
  because the fast path never enrolls. The 1.2 ns difference is within layout noise.
- **PQ bypass (D1+D2+D4, timestamps kept)** removes **72.5 ns** (baseline 127.1 →
  fast_path 54.6), 112 % of the tax: FC-PQ ends up 8.1 ns *cheaper* than FC.
  12.7 ns of that is the enrollment thread-id handled above. The remaining
  ≈59.8 ns is the rest of the combine protocol at occupancy 1: ring push/drain, PQ
  push/pop/re-push/second pop, buffer, pass counter, and payload publish/complete
  stores.
- **Timestamps + usage charge** cost **21.2 ns** on the fast path (fast_path 54.6 →
  fast_path_notime 33.4). This matches the TSC study's V0 prediction ("no timing":
  20.2 ns/request), so notime skips nothing beyond the timing and accounting.
- **Hit counter (stat)** costs +0.2 ns (55.8 → 56.0).
- Residual timed fast path: 54.6 ns, about 33 ns of CAS + delegate call + unlock plus
  21 ns of accounting. FC's own path costs 62 ns.

## 4. Throughput and fairness under load

FC-PQ throughput is Mops/s, with FC-PQ/FC paired ratios. "net" is serial ns/op after
subtracting hold (§1). "Δ vs baseline" is FC-PQ throughput within the same tag.

| cell | T | variant | tag | FC-PQ Mops/s | FC-PQ/FC | FC-PQ net ns/op | Δ vs baseline | FC-PQ Jain | hit rate |
|---|---:|---|---|---|---|---:|---:|---|---|
| b-lo | 2 | baseline | abl | 0.076 [0.075, 0.076] | 0.991 | — | — | 0.9909 | — |
| b-lo | 2 | cached_tid | abl | 0.073 [0.070, 0.073] | 0.949 | — | −4.0 % | 0.9986 | — |
| b-lo | 2 | fast_path | abl | 0.077 [0.077, 0.077] | 1.004 | — | +1.3 % | 0.9999 | — |
| b-lo | 2 | fast_path+cached_tid | abl | 0.077 [0.077, 0.077] | 1.004 | — | +1.3 % | 0.9997 | — |
| b-lo | 4 | baseline | abl | 0.151 [0.150, 0.151] | 0.998 | — | — | 0.9843 | — |
| b-lo | 4 | cached_tid | abl | 0.146 [0.145, 0.148] | 0.965 | — | −3.3 % | 0.9915 | — |
| b-lo | 4 | fast_path | abl | 0.154 [0.154, 0.154] | 1.015 | — | +2.0 % | 0.9982 | — |
| b-lo | 8 | baseline | short | 0.302 [0.302, 0.303] | 1.011 | — | — | 0.9971 | — |
| b-lo | 8 | fast_path+cached_tid | short | 0.307 [0.307, 0.307] | 1.029 | — | +1.7 % | 0.9993 | — |
| b-lo | 8 | fast_path+cached_tid+stat | short | 0.307 [0.306, 0.307] | 1.025 | — | +1.7 % | 0.9958 | **0.979** [0.976, 0.981] |
| b-lo | 8 | fast_path | abl | 0.308 [0.307, 0.308] | 1.032 | — | +1.7 % (vs abl 0.303) | 0.9999 | — |
| b-mid | 2 | baseline | abl | 0.658 [0.653, 0.661] | 0.937 | — | — | 0.9983 | — |
| b-mid | 2 | fast_path | abl | 0.719 [0.716, 0.723] | 1.033 | — | **+9.3 %** | 1.0000 | — |
| b-mid | 4 | baseline | abl | 1.210 [1.175, 1.222] | 0.967 | — | — | 0.9995 | — |
| b-mid | 4 | fast_path | abl | 1.443 [1.390, 1.447] | 1.174 | — | **+19.3 %** | 1.0000 | — |
| b-mid | 8 | baseline | short | 2.062 [2.049, 2.084] | 0.921 | — | — | 1.0000 | — |
| b-mid | 8 | fast_path+cached_tid | short | 1.894 [1.891, 1.936] | 0.843 | — | **−8.1 %** | 1.0000 | — |
| b-mid | 8 | fast_path+cached_tid+stat | short | 1.954 [1.949, 1.983] | 0.865 | — | −5.2 % | 1.0000 | **0.112** [0.109, 0.114] |
| b-mid | 8 | fast_path | abl | 1.936 [1.933, 1.964] | 0.859 | — | **−6.8 %** (vs abl 2.078) | 1.0000 | — |
| c-cs1 | 32 | baseline | short | 7.228 [7.199, 7.268] | 0.687 | 125.0 | — | 1.0000 | — |
| c-cs1 | 32 | fast_path+cached_tid | short | 6.979 [6.963, 6.981] | 0.620 | 130.0 | **−3.4 %** | 1.0000 | — |
| c-cs1 | 32 | baseline / cached_tid / fast_path | abl | 7.180 / 7.238 / 7.075 | 0.650 / 0.665 / 0.653 | 126.0 / 124.9 / 128.0 | — / +0.8 % / −1.5 % | 1.0000 | — |
| c-cs1k | 32 | baseline | short | 1.000 [0.998, 1.010] | 0.881 | 147.7 | — | 1.0000 | — |
| c-cs1k | 32 | fast_path+cached_tid | short | 1.018 [1.012, 1.027] | 0.898 | 145.2 | +1.8 % | 1.0000 | — |
| c-cs1k | 32 | baseline / cached_tid / fast_path | abl | 1.008 / 1.073 / 1.164 | 0.878 / 0.990 / 0.994 | 147.5 / 147.1 / 145.3 | layout† | 1.0000 | — |
| d-het | 8 | baseline | short | 0.784 [0.766, 0.785] | 1.960 | 125.5 | — | **0.8930** [0.8873, 0.9127] | — |
| d-het | 8 | fast_path | short | 0.811 [0.792, 0.816] | 1.984 | 129.2 | +3.4 % | **0.8924** [0.8726, 0.9157] | — |
| d-het | 8 | fast_path+cached_tid | short | 0.623 [0.563, 0.745] | 1.592 | 128.4 | −21 %‡ | **0.8691** [0.8332, 0.8780]‡ | — |
| d-het | 8 | fast_path_notime | short | 0.682 [0.620, 0.774] | 1.678 | 138.1 | −13 % | **0.8394** [0.8018, 0.8832] | — |
| d-het | 8 | baseline / cached_tid / fast_path | abl | 0.759 / 0.774 / 0.826 | 1.915 / 2.028 / 2.034 | 125.1 / 131.1 / 126.2 | — | **0.9002 / 0.8995 / 0.9012** | — |
| d-het | 32 | baseline / cached_tid / fast_path | abl | 0.579 / 0.600 / 0.645 | 1.998 / 2.137 / 2.171 | 143.6 / 142.4 / 141.2 | — | **0.9312 / 0.9345 / 0.9313** | — |

FC reference Jain: 0.62-0.63 in d-het at both 8W and 32W, and 0.98-1.00 elsewhere.

† At c-cs1k the absolute throughput follows the binary's CS-loop codegen: hold/op
is 713-844 ns across these three binaries. Net overhead is flat at 145-148 ns.

‡ See §5. Throughput in d-het follows the served mix: FC-PQ serves more short
requests than FC, which is why FC-PQ/FC ≈ 2. A variant that gives the 8000-loop
threads more service therefore loses op/s without costing more per op. Net ns/op is
flat at 125-131 ns except for notime.

## 5. Readings

- **32 workers, zero non-CS.** The largest regression is small and confined to the
  tiny CS: c-cs1 serial cost rises by 4-5 ns/op for fast_path+cached_tid (−3.4 %
  op/s) and by 2 ns for fast_path (−1.5 %, `abl`). At c-cs1k the change is between
  +1.8 % and zero (−2 ns/op). [INFERENCE] The cost is the extra contended `try_lock`
  that every request now issues before enrolling.
- **Fairness (1:8 cost).** fast_path vs baseline Jain is unchanged within noise:
  8W 0.8924 vs 0.8930 (`short`) and 0.9012 vs 0.9002 (`abl`); 32W 0.9313 vs 0.9312.
  fast_path_notime degrades 8W Jain to 0.839 [0.802, 0.883]. In 2 of 3 trials the
  8000-loop threads got 1.40-1.49× the mean share, against 1.20-1.23× in the
  baseline. That is the expected cost of not charging fast-path service.
- **Unresolved: fast_path+cached_tid d-het 8W.** Its Jain of 0.869 [0.833, 0.878]
  lies below the baseline range in 2 of 3 trials, with the long threads again getting
  1.33-1.43× share. cached_tid changes only the enrollment tie-breaker id, so by
  construction this should equal fast_path. The two anomalous cells, fp+ct and
  notime, ran back to back (00:46:11-00:46:29), and the third trial of each looks like
  the baseline. No sibling cargo ran in the window, but other agents' measurements on
  this machine were not monitored. **This cell needs a ≥ 5-trial rerun before
  fast_path+cached_tid is declared fairness-neutral**, and a d-het run with the stat
  build would show whether the fast path fires there at all. Separately, every
  variant *including the baseline* leaves two of the four cs-1000 threads at
  ≈0.35-0.49 share at 8W. That pre-existing unfairness is why baseline Jain is 0.89,
  not ~1.
- **Low contention.** With large non-CS (b-lo), the hit rate is 97.9 % at 8W and
  FC-PQ throughput rises 1.3-2.0 % at 2/4/8W. At moderate contention (b-mid,
  ≈1.4 µs between requests) the fast path gains +9 % at 2W and +19 % at 4W, **but
  loses 5-8 % at 8W**, where the hit rate is only 11.2 %. That 8W cell is the one
  regression larger than noise. [INFERENCE] When the gate fails with the node
  inactive, the request pays try_lock + release + enroll + try_lock again. The
  resulting lock ping-pong costs more than 11 % of hits saves.
- **cached_tid alone** at b-lo 2/4/8W is 3-4 % *slower* than baseline. FC in the same
  binary is unchanged, and net overhead is not computable here (non-CS dominated).
  The 1W and 32W cells show cached_tid neutral-to-positive, so this is probably
  layout of the non-CS loop [INFERENCE], but it is unverified.

## 6. Missing cells (trimmed on Main's instruction, 2026-09-28 00:40)

- b-lo 4W for fast_path+cached_tid: that run was killed, and its partial output was
  deleted.
- b-lo / b-mid 2W and 4W for fast_path+cached_tid, fast_path_notime, and stat: so
  there is no hit rate at 2/4W.
- b-mid 2/4/8W for cached_tid is present in `abl` (§raw) but not tabulated. The
  values are 0.646 / 1.183 / 2.064 Mops/s, i.e. −1.8 % / −2.2 % / −0.7 % vs baseline.
- c-cs1 / c-cs1k for fast_path_notime and stat.
- d-het 32W for fast_path+cached_tid and fast_path_notime. d-het 8W for these two has
  only 3 trials, and those need the rerun above.
- `abl-r2`, a baseline repeat after the sweep, was cancelled. Noise evidence comes
  instead from four baseline measurements: `cpu0-r1`, `cpu0-r2` (unpinned), `abl`
  and `short` (pinned).

## 7. Noise

Baseline 1W a-tiny FC-PQ: 7.305 / 7.468 / 7.249 / 7.316 Mops/s (cpu0-r1 / cpu0-r2 /
abl / short). Net overhead: 127.1 / 124.3 / 128.3 / 127.1 ns. Paired ratio: 0.539 /
0.538 / 0.526 / 0.528. Within-run min-max spread is ≤ 1 % pinned and ≤ 4 % unpinned.

Unpinned multi-worker cells are noisier: cpu0 c-cs1 FC ranges from 5.5 to 11.7
Mops/s, and the b-mid 2W baseline ranges from 0.487 to 0.660. Pinned `abl` cells
spread ≤ 4 % except FC c-cs1 (10.1-11.5). Across the four baseline measurements the
saturated FC-PQ net ns/op medians are: c-cs1 124.0-128.6 (cpu0-r2 has one 167.5 ns
outlier trial); c-cs1k 144.4-147.7; d-het 32W 142.0-143.6; d-het 8W 125.1-129.2.
Pinned repeats (`abl` vs `short`) agree within ±1.2 ns. Deltas below ≈3 ns/op or ≈3 %
in op/s are not distinguishable from noise.

## 8. Recommendation

Keep the fast path with timestamps and charging (`fcpq_fast_path`). At 1W it
removes the whole 65 ns tax: FC-PQ/FC is 1.12 at tiny CS and 1.01 at cs 1000, above
the 0.95 target. It leaves 32W serial cost within 2-5 ns/op and 1:8 service Jain
unchanged (8W and 32W, fast_path vs baseline).

`fcpq_cached_tid` is optional. It is worth 13 ns on every slow-path enrollment,
is neutral under saturation, and removes a Thread clone per enrollment. Once the
fast path is on, though, it adds nothing measurable at 1W. The only build that
misses the raw a-1k target is fast_path+cached_tid, and it has the unresolved d-het
cell, so the measured recommendation is **plain `fcpq_fast_path`**. Add cached_tid
only after the d-het rerun, and treat its a-1k miss as a code-layout effect that
should be re-checked in the final build.

Do not keep `fcpq_fast_path_notime`. Its extra 21 ns is exactly the timestamp cost
the TSC study predicts, and it measurably skews 1:8 service. Keep
`fcpq_fast_path_stat` only as a diagnostic (+0.2 ns).

Before making this the default, three items remain:

1. Rerun d-het 8W for fast_path+cached_tid with ≥ 5 trials (§5, unresolved).
2. Address the 8W b-mid −5-8 % regression. One option is to skip the pre-enroll
   try_lock when the previous request did not take the fast path; that would be a new
   design decision for the plan.
3. Check whether the c-cs1 +4-5 ns/op at 32W is the same mechanism.

## 9. Exact commands

```sh
# builds (one CARGO_TARGET_DIR; the binary is copied per variant)
python3 docs/evidence/e0b-fast-path-ablation/runner.py build baseline cached_tid fast_path \
  fast_path+cached_tid fast_path_notime fast_path+cached_tid+stat
#   = CARGO_TARGET_DIR=.worktree/bench-target cargo build --release -p dlock --features <set>
#   (root Cargo.toml forwards fcpq_* to libdlock)
# abl sweep (stopped at fast_path+cached_tid b-lo 4W)
python3 docs/evidence/e0b-fast-path-ablation/runner.py run baseline cached_tid fast_path \
  fast_path+cached_tid fast_path_notime fast_path+cached_tid+stat --tag abl
# trimmed matrix
docs/evidence/e0b-fast-path-ablation/run_trimmed.sh
# tables + raw CSVs
python3 docs/evidence/e0b-fast-path-ablation/runner.py summarize
```

The per-cell command is recorded in `raw/cells.jsonl`, e.g.:
`taskset -c 48-48 .worktree/bins/dlock-fast_path d-lock2 -t 1 -d 2 --warmup 1 --trials 3 -o … -l fc,fc-pq-b-heap counter-proportional --cs 1 --non-cs 0 --file-name a-tiny`.
The `cpu0-*` runs had no `taskset`.

## 10. Machine and load

- CPU: Intel Xeon Gold 6438M, 2 sockets × 32 cores × SMT2 = 128 logical CPUs.
  NUMA node0 = 0-31,64-95; node1 = 32-63,96-127.
- Kernel 6.17.7. `schedutil` governor, turbo enabled. TSC 2200.2 MHz.
- `uptime` before and after every cell is in `raw/cells.jsonl`. At the start of the
  `short` 1W cells the 1-minute load was 1.1-1.5. During and after 32W cells it rose
  to 10-30, which is self-induced: 32 spinning workers.
- Background load: rust-analyzer and omp agents, ≤ 25 % of one CPU in total. The
  FastPathImpl sibling ran no cargo during the announced windows (22:48-23:20 and
  23:45-00:48). TscAccuracy-2 / SpinThenPark-2 activity in other checkouts was not
  monitored.

## 11. Files

- `runner.py`: builds, runs, and summarizes. `run_trimmed.sh`: the trimmed matrix.
- Raw data is not committed. It is written to the ignored `.worktree/e0b/raw/` and
  archived at `~/Locks-artifacts/e0b-fcpq-fast-path/raw/`:
- `raw/trials.csv`: one row per (tag, variant, cell, lock, trial), with Mops/s, Jain,
  hold/op, and fast-path hits.
- `raw/threads.csv`: per-thread rows with acquisitions, hold time, and normalized share.
- `raw/cells.jsonl`: per-cell command, uptime, and binary digest.
- `raw/summary.md`: the full generated table.
- `raw/builds/*.json`: features and source digest per binary.
- Arrow files and stdout are under the ignored `.worktree/e0b/<tag>/<variant>/<cell>/t<T>/`.

Harness changes (`src/benchmark/**`, all `#[cfg(feature = "fcpq_fast_path_stat")]`):
- `Records.all_acquire` counts all acquisitions including warmup.
- `proportional_counter` prints `Fast path hits: <n> acquisitions: <m> (trial k)`
  for FC-PQ (BinaryHeap).
