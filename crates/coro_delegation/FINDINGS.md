# Findings

Newest first. Every number is a median over 3 repeats with `[min, max]`
unless stated; differences inside the spread are not interpreted. Tables are
produced by `python3 scripts/summarize.py results 'matrix-*.json'` (phase 2) and
`python3 scripts/summarize.py results 'p3-*.json'` (phase 3).

## 2026-09-28 — Cross-runtime baseline: tokio locks

### Setup

New crate `crates/coro_tokio_baseline`, binary `tokio-bench`: this study's
workload on tokio 1.53.1's multi-thread runtime (`worker_threads = W`, worker
thread i pinned to logical CPU i = one per physical core; every JSON records
`pinned_cpus = 0..W-1`, `runtime_threads = W`), default runtime settings.
Same workload code as `coro-bench` (copied, not re-derived): BTreeMap insert
over key space 65 536 from the same per-client xorshift stream, light CS 1 000
TSC cycles + insert, heavy 8×, sustained 64 clients / 4 000-cycle parallel
work, bursty 16 clients / 32 000, W bystanders spinning 1 000 cycles then
`tokio::task::yield_now()`, 200 ms warm-up, 2 s window, rdtscp, same
histogram/quantiles, Jain over per-client service cycles. Tasks are spawned
from the main thread (tokio has no `spawn_on`). Locks: `tokio-mutex`
(`tokio::sync::Mutex`), `tokio-mutex-unconstrained` (same, each client loop
wrapped in `tokio::task::unconstrained`, which removes tokio's coop budget:
128 units per poll, one consumed per completed `tokio::sync::Mutex` acquire,
so an available lock can never be refused with a forced `Pending`),
`async-lock` (`async_lock::Mutex` 3.4.2), `std-mutex` and `parking-lot`
(0.12.5), both locked and waited on inside the task. `tokio-bench --sanity`
(map len = total ops, unique keys): PASS for all five.

Matrix: 5 locks × W {8, 16} × {sustained, bursty} × 3 repeats = 60 runs
(`results/tokio-<lock>-w<W>-h8-<sus|bur>-r<i>.json`), then the existing
`target/release/coro-bench` (mtime 09:25:54 UTC, sha256 `95eb5757…`, not
rebuilt) for `dispatch`, `dispatch-home`, `ces-k64-home`, `fc-remote`,
`fcpq-h16-home` in the same cells, balance 31, 3 repeats = 60 runs
(`results/xrt-<label>-w<W>-h8-b31-<sus|bur>-r<i>.json`). Sequential, repeats
outermost, unix 1790627987–1790628742, holding
`~/.cache/locks-experiments/measurement.lock`, 20 s idle after every
std-mutex / parking-lot run (caveat 7). Driver:
`crates/coro_tokio_baseline/scripts/run_xrt.sh`; table:
`python3 scripts/summarize_tokio.py results`. Superseded sets:
`results/xrt-v1-redb-overlap.tar.zst` (run without the lock while another
job measured on CPUs 16–23), `results/xrt-v2-turbo-carryover.tar.zst` (no
cooldown; caveat 7).

### Results (medians [min, max], n = 3)

| workers | contention | runtime | variant | n | throughput (Mops/s) | thr / tokio-mutex | service Jain | light run p50 / p99 (µs) | heavy run p50 / p99 (µs) | starved clients |
|---|---|---|---|---|---|---|---|---|---|---|
| 8 | sus | tokio | tokio-mutex | 3 | 0.241 [0.241, 0.242] | 1.00 | 0.663 [0.662, 0.663] | 253.2 [253.2, 253.2] / 268.1 [253.2, 268.1] | 253.2 [253.2, 253.2] / 268.1 [253.2, 268.1] | 0 |
| 8 | sus | tokio | tokio-mutex-unconstrained | 3 | 0.239 [0.239, 0.239] | 0.99 | 0.665 [0.664, 0.665] | 253.2 [253.2, 253.2] / 268.1 [268.1, 268.1] | 253.2 [253.2, 253.2] / 268.1 [268.1, 268.1] | 0 |
| 8 | sus | tokio | async-lock | 3 | 0.177 [0.177, 0.239] | 0.73 | 0.016 [0.016, 0.661] | 253.2 / 268.1 | 3.7 [3.7, 253.2] / 3.7 [3.7, 268.1] | 63 [0, 63] |
| 8 | sus | tokio | std-mutex | 3 | 0.204 [0.203, 0.230] | 0.85 | 0.055 [0.047, 0.057] | 42.8 [33.5, 42.8] / 417.0 [268.1, 476.6] | 5.4 [5.4, 5.6] / 253.2 [201.1, 297.9] | 56 |
| 8 | sus | tokio | parking-lot | 3 | 0.140 [0.138, 0.142] | 0.58 | 0.048 [0.048, 0.049] | 57.7 [31.6, 119.1] / 3217.0 [2978.7, 3336.1] | 5.4 [5.4, 5.8] / 655.3 [655.3, 685.1] | 56 |
| 8 | sus | coro | dispatch | 3 | 0.218 [0.217, 0.218] | 0.90 | 0.659 [0.659, 0.659] | 283.0 [283.0, 283.0] / 297.9 [297.9, 297.9] | 283.0 [283.0, 283.0] / 297.9 [297.9, 297.9] | 0 |
| 8 | sus | coro | dispatch-home | 3 | 0.253 [0.253, 0.253] | 1.05 | 0.669 [0.669, 0.669] | 238.3 [238.3, 238.3] / 253.2 [253.2, 253.2] | 238.3 [238.3, 238.3] / 253.2 [253.2, 253.2] | 0 |
| 8 | sus | coro | ces-k64-home | 3 | 0.381 [0.380, 0.381] | 1.58 | 0.659 [0.659, 0.659] | 163.8 [163.8, 163.8] / 171.3 [171.3, 178.7] | 163.8 [163.8, 163.8] / 171.3 [171.3, 171.3] | 0 |
| 8 | sus | coro | fc-remote | 3 | 0.387 [0.387, 0.388] | 1.61 | 0.653 [0.653, 0.653] | 156.4 [156.4, 156.4] / 171.3 [171.3, 171.3] | 156.4 [156.4, 156.4] / 171.3 [171.3, 171.3] | 0 |
| 8 | sus | coro | fcpq-h16-home | 3 | 0.571 [0.569, 0.571] | 2.37 | 0.930 [0.930, 0.930] | 63.3 [63.3, 67.0] / 148.9 [148.9, 148.9] | 238.3 [238.3, 253.2] / 342.5 [342.5, 342.5] | 0 |
| 8 | bur | tokio | tokio-mutex | 3 | 0.073 [0.073, 0.073] | 1.00 | 0.668 [0.668, 0.669] | 201.1 [201.1, 201.1] / 201.1 [201.1, 201.1] | 201.1 [201.1, 201.1] / 201.1 [201.1, 201.1] | 0 |
| 8 | bur | tokio | tokio-mutex-unconstrained | 3 | 0.073 [0.073, 0.073] | 1.00 | 0.669 [0.669, 0.670] | 201.1 [201.1, 201.1] / 201.1 [201.1, 201.1] | 201.1 [201.1, 201.1] / 201.1 [201.1, 201.1] | 0 |
| 8 | bur | tokio | async-lock | 3 | 0.054 [0.054, 0.054] | 0.75 | 0.062 | – / – | 3.7 [3.7, 3.7] / 3.7 [3.7, 3.7] | 15 |
| 8 | bur | tokio | std-mutex | 3 | 0.366 [0.365, 0.384] | 5.02 | 0.255 [0.253, 0.256] | 5.6 [5.6, 6.1] / 46.5 [39.1, 48.4] | 4.4 [4.4, 4.4] / 21.4 [18.6, 23.3] | 8 |
| 8 | bur | tokio | parking-lot | 3 | 0.357 [0.356, 0.359] | 4.90 | 0.250 [0.250, 0.250] | 5.6 [5.6, 5.6] / 55.9 [54.0, 55.9] | 4.7 [4.7, 4.7] / 26.1 [26.1, 26.1] | 8 |
| 8 | bur | coro | dispatch | 3 | 0.071 [0.071, 0.071] | 0.97 | 0.674 [0.674, 0.674] | 208.5 [208.5, 208.5] / 238.3 [238.3, 238.3] | 208.5 [208.5, 208.5] / 238.3 [238.3, 238.3] | 0 |
| 8 | bur | coro | dispatch-home | 3 | 0.147 [0.146, 0.150] | 2.01 | 0.669 [0.669, 0.670] | 93.1 [89.4, 93.1] / 126.6 [126.6, 126.6] | 93.1 [89.4, 93.1] / 126.6 [126.6, 126.6] | 0 |
| 8 | bur | coro | ces-k64-home | 3 | 0.317 [0.314, 0.318] | 4.36 | 0.678 [0.677, 0.679] | 39.1 [39.1, 39.1] / 59.6 [59.6, 59.6] | 39.1 [39.1, 39.1] / 59.6 [59.6, 59.6] | 0 |
| 8 | bur | coro | fc-remote | 3 | 0.307 [0.307, 0.308] | 4.22 | 0.655 [0.655, 0.656] | 39.1 [39.1, 39.1] / 55.9 [55.9, 55.9] | 39.1 [39.1, 39.1] / 55.9 [55.9, 55.9] | 0 |
| 8 | bur | coro | fcpq-h16-home | 3 | 0.365 [0.365, 0.366] | 5.01 | 0.685 [0.685, 0.686] | 21.4 [21.4, 21.4] / 67.0 [67.0, 67.0] | 29.8 [29.8, 29.8] / 78.2 [74.5, 78.2] | 0 |
| 16 | sus | tokio | tokio-mutex | 3 | 0.232 [0.231, 0.233] | 1.00 | 0.666 [0.665, 0.668] | 268.1 [268.1, 268.1] / 268.1 [268.1, 283.0] | 268.1 [268.1, 268.1] / 268.1 [268.1, 283.0] | 0 |
| 16 | sus | tokio | tokio-mutex-unconstrained | 3 | 0.231 [0.230, 0.231] | 0.99 | 0.667 [0.667, 0.667] | 268.1 [268.1, 268.1] / 283.0 [283.0, 283.0] | 268.1 [268.1, 268.1] / 283.0 [283.0, 283.0] | 0 |
| 16 | sus | tokio | async-lock | 3 | 0.176 [0.176, 0.176] | 0.76 | 0.016 | – / – | 3.7 [3.7, 3.7] / 3.7 [3.7, 3.7] | 63 |
| 16 | sus | tokio | std-mutex | 3 | 0.167 [0.166, 0.167] | 0.72 | 0.234 [0.234, 0.234] | 2.0 [1.9, 2.0] / 1310.6 [1310.6, 1370.2] | 5.1 [5.1, 5.1] / 1608.5 [1608.5, 1608.5] | 48 |
| 16 | sus | tokio | parking-lot | 3 | 0.108 [0.108, 0.114] | 0.46 | 0.197 [0.181, 0.198] | 89.4 [63.3, 89.4] / 1489.4 [1429.8, 1548.9] | 8.4 [8.4, 8.4] / 1251.1 [1131.9, 1251.1] | 48 |
| 16 | sus | coro | dispatch | 3 | 0.214 [0.214, 0.215] | 0.92 | 0.663 [0.663, 0.663] | 283.0 [283.0, 283.0] / 297.9 [297.9, 297.9] | 283.0 [283.0, 283.0] / 297.9 [297.9, 297.9] | 0 |
| 16 | sus | coro | dispatch-home | 3 | 0.239 [0.239, 0.241] | 1.03 | 0.674 [0.673, 0.674] | 253.2 [253.2, 253.2] / 268.1 [268.1, 268.1] | 253.2 [253.2, 253.2] / 268.1 [268.1, 268.1] | 0 |
| 16 | sus | coro | ces-k64-home | 3 | 0.372 [0.372, 0.372] | 1.60 | 0.661 [0.661, 0.662] | 163.8 [163.8, 163.8] / 178.7 [178.7, 178.7] | 163.8 [163.8, 163.8] / 178.7 [178.7, 178.7] | 0 |
| 16 | sus | coro | fc-remote | 3 | 0.378 [0.378, 0.379] | 1.63 | 0.659 [0.659, 0.660] | 163.8 [163.8, 163.8] / 178.7 [171.3, 178.7] | 163.8 [163.8, 163.8] / 178.7 [178.7, 178.7] | 0 |
| 16 | sus | coro | fcpq-h16-home | 3 | 0.547 [0.545, 0.549] | 2.36 | 0.933 [0.932, 0.934] | 70.7 [70.7, 70.7] / 141.5 [141.5, 141.5] | 253.2 [253.2, 253.2] / 342.5 [342.5, 342.5] | 0 |
| 16 | bur | tokio | tokio-mutex | 3 | 0.072 [0.072, 0.072] | 1.00 | 0.672 [0.671, 0.674] | 201.1 [201.1, 201.1] / 208.5 [208.5, 208.5] | 201.1 [201.1, 201.1] / 208.5 [208.5, 208.5] | 0 |
| 16 | bur | tokio | tokio-mutex-unconstrained | 3 | 0.072 [0.072, 0.072] | 1.00 | 0.672 [0.672, 0.672] | 201.1 [201.1, 201.1] / 208.5 [208.5, 208.5] | 201.1 [201.1, 201.1] / 208.5 [208.5, 208.5] | 0 |
| 16 | bur | tokio | async-lock | 3 | 0.054 [0.054, 0.054] | 0.75 | 0.062 | – / – | 3.7 [3.7, 3.7] / 3.7 [3.7, 4.0] | 15 |
| 16 | bur | tokio | std-mutex | 3 | 0.195 [0.172, 0.228] | 2.70 | 0.782 [0.767, 0.807] | 5.6 [5.4, 5.8] / 625.5 [357.4, 834.0] | 5.8 [5.8, 7.2] / 625.5 [342.6, 744.7] | 0 |
| 16 | bur | tokio | parking-lot | 3 | 0.125 [0.122, 0.154] | 1.73 | 0.774 [0.692, 0.775] | 39.1 [29.8, 63.3] / 1131.9 [744.7, 1191.5] | 8.8 [8.4, 8.8] / 1012.8 [685.1, 1012.8] | 0 |
| 16 | bur | coro | dispatch | 3 | 0.071 [0.071, 0.071] | 0.98 | 0.679 [0.679, 0.679] | 208.5 [208.5, 208.5] / 238.3 [238.3, 238.3] | 208.5 [208.5, 208.5] / 238.3 [238.3, 238.3] | 0 |
| 16 | bur | coro | dispatch-home | 3 | 0.172 [0.161, 0.172] | 2.39 | 0.673 [0.672, 0.673] | 74.5 [74.5, 78.2] / 111.7 [111.7, 126.6] | 74.5 [74.5, 78.2] / 111.7 [111.7, 126.6] | 0 |
| 16 | bur | coro | ces-k64-home | 3 | 0.350 [0.349, 0.350] | 4.85 | 0.663 [0.663, 0.663] | 29.8 [29.8, 29.8] / 44.7 [44.7, 44.7] | 29.8 [29.8, 29.8] / 44.7 [44.7, 44.7] | 0 |
| 16 | bur | coro | fc-remote | 3 | 0.358 [0.357, 0.358] | 4.97 | 0.662 [0.662, 0.662] | 28.9 [28.9, 28.9] / 50.3 [50.3, 52.1] | 28.9 [28.9, 28.9] / 50.3 [48.4, 50.3] | 0 |
| 16 | bur | coro | fcpq-h16-home | 3 | 0.392 [0.392, 0.392] | 5.44 | 0.726 [0.724, 0.727] | 15.8 [15.8, 15.8] / 54.0 [54.0, 54.0] | 33.5 [33.5, 33.5] / 67.0 [67.0, 70.7] | 0 |

Ratios to `std-mutex` (medians): `ces-k64-home` / `fc-remote` /
`fcpq-h16-home` = 1.86 / 1.90 / 2.79 (w8 sus), 0.87 / 0.84 / 1.00 (w8 bur;
fcpq-h16-home inside std-mutex's spread), 2.23 / 2.27 / 3.28 (w16 sus),
1.80 / 1.84 / 2.01 (w16 bur). std-mutex and parking-lot starve all W
bystanders in every run; every other row starves none, except async-lock
w8 bur r2 (1) and w8 sus r3 (2) (`starved_bystanders`, censored at 2.196 s).

### Side check: tokio's LIFO slot (not part of the matrix)

`Builder::disable_lifo_slot()` needs `--cfg tokio_unstable`, so this used a
throwaway copy of `tokio-bench` built with that flag (its LIFO-on runs are
0.96–1.00× the release binary's medians), same window (unix
1790628744–1790628853), LIFO on vs off in that build, n = 3. JSONs, patch
and probe scripts for this section and caveats 2 and 7:
`results/xrt-side-lifo.tar.zst`.

| workers | contention | lock | LIFO slot | throughput (Mops/s) | service Jain | starved clients | bystander p99 (µs) |
|---|---|---|---|---|---|---|---|
| 8 | sus | tokio-mutex | on | 0.231 [0.230, 0.242] | 0.675 [0.662, 0.677] | 0 | 18.6 [17.7, 19.5] |
| 8 | sus | tokio-mutex | off | 0.250 [0.243, 0.251] | 0.689 [0.688, 0.693] | 0 | 18.6 [18.6, 19.5] |
| 8 | sus | async-lock | on | 0.176 [0.176, 0.403] | 0.016 | 63 | 2.2 [2.0, 2.2] |
| 8 | sus | async-lock | off | 0.237 [0.233, 0.241] | 0.695 [0.690, 0.697] | 0 | 19.5 [18.6, 19.5] |
| 8 | bur | tokio-mutex | on | 0.072 [0.072, 0.073] | 0.679 [0.668, 0.680] | 0 | 1.5 [1.3, 1.6] |
| 8 | bur | tokio-mutex | off | 0.263 [0.261, 0.265] | 0.676 [0.675, 0.679] | 0 | 44.7 [42.8, 44.7] |
| 8 | bur | async-lock | on | 0.072 [0.054, 0.072] | 0.665 [0.062, 0.667] | 0 [0, 15] | 1.0 [1.0, 2.3] |
| 8 | bur | async-lock | off | 0.250 [0.249, 0.253] | 0.682 [0.678, 0.683] | 0 | 42.8 [42.8, 44.7] |
| 16 | sus | tokio-mutex | on | 0.229 [0.229, 0.232] | 0.671 [0.666, 0.673] | 0 | 17.7 [6.5, 18.6] |
| 16 | sus | tokio-mutex | off | 0.248 [0.245, 0.253] | 0.687 [0.687, 0.691] | 0 | 24.2 [23.3, 24.2] |
| 16 | sus | async-lock | on | 0.176 | 0.016 | 63 | 5.4 |
| 16 | sus | async-lock | off | 0.234 [0.232, 0.244] | 0.703 [0.692, 0.704] | 0 | 25.1 [23.3, 25.1] |
| 16 | bur | tokio-mutex | on | 0.072 | 0.671 [0.671, 0.672] | 0 | 4.2 |
| 16 | bur | tokio-mutex | off | 0.248 [0.246, 0.251] | 0.693 [0.690, 0.693] | 0 | 46.5 |
| 16 | bur | async-lock | on | 0.054 [0.054, 0.072] | 0.062 [0.062, 0.675] | 15 [0, 15] | 5.4 [4.2, 6.3] |
| 16 | bur | async-lock | off | 0.237 [0.235, 0.244] | 0.702 [0.699, 0.704] | 0 | 46.5 |

Across the matrix and this check, async-lock collapsed to a single client
in 20 of 24 LIFO-on runs and 0 of 12 LIFO-off runs. With the same build's
unstable metrics (w8, n = 1 per cell): `budget_forced_yield_count` = 0 in all
eight runs (tokio-mutex and -unconstrained, LIFO on and off); steals per
worker per window 4.4–16 k with the slot on, 127–340 k with it off.

### Caveats

1. **tokio LIFO slot.** A wake issued on a worker goes into that worker's
   LIFO slot, polled after the current poll returns (at most 3 per tick,
   inside the parent poll's coop budget) and never stolen. Mutex handoff and
   async-lock notify wakes therefore land on the unlocker's worker, like coro
   `dispatch` default placement (back of the unlocker's FIFO queue), not
   `home`. [INFERENCE, supported by the side check] the new owner waits for
   the unlocker's poll to end, i.e. behind its parallel work: disabling the
   slot lifts tokio-mutex 3.4–3.7× bursty and 1.08× sustained, and ends the
   async-lock collapse (its notified waiter is never polled, so the 0.5 ms
   starvation handoff never fires, and the releaser re-takes the free lock
   after its own synchronous parallel work).
2. **Coop budget.** Only `tokio::sync::Mutex`'s acquire consumes budget
   (async-lock, std, parking_lot and `yield_now` do not), and no forced
   yield occurred in any diagnostic run, so `unconstrained` has nothing to
   remove here: 1.00× bursty, 0.99× at w16 sus inside the spread, 0.99× at w8
   sus outside it (0.239 vs 0.241 [0.241, 0.242]; 0.233–0.237 vs 0.241–0.242
   in an 8-repeat probe), mechanism of that 1–3 % not identified.
3. **No balancing steal.** tokio never steals from a busy peer; the coro
   cells here run balance 31 (a busy worker takes half of a random peer's
   queue every 31 polls).
4. **Different steal semantics.** A tokio worker steals half of a random
   peer's run queue (never its LIFO slot) whenever its own slot and queue are
   empty, and `yield_now` parks the task in a defer list that does not count
   as work (woken at the next maintenance tick, every 61 polls, or after a
   failed steal). A worker holding only yielded bystanders therefore steals,
   whereas in the coro executor bystanders are always runnable and
   steal-when-idle never fires (the standing limitation of this study, which
   still applies to the coro rows). Hence bystander delay (`bystander_latency`
   in the tokio JSON: yield → next poll) is not comparable to coro's
   schedule→poll metric and is compared only within tokio above. True parks
   (`tokio_workers[].parks`, summed over workers) stay ≤ 13 per run except
   async-lock w8 sus r3 (285 693).
5. **A blocking mutex holds the worker.** A waiting std / parking_lot client
   blocks its OS thread, and a client whose `lock()` returned never yields
   (the loop's only await is the lock). The first W clients polled keep the W
   workers for the whole run: starved clients = clients − W (56 / 48
   sustained, 8 / 0 bursty) plus all W bystanders. Those rows measure W
   threads on a futex / parking_lot mutex, not a 64- or 16-client service;
   ratios against them are not like-for-like.
6. **One await per iteration.** [INFERENCE, untested] a service that awaits
   I/O between critical sections would return the worker to the scheduler,
   removing caveat 5 and probably the async-lock collapse.
7. **Turbo carry-over after blocking-lock runs (machine).** For 5–15 s after
   a std-mutex / parking-lot run, CPUs 0–7 report 2.2 GHz instead of 3.69 GHz
   (`scaling_cur_freq`, intel_cpufreq + schedutil); the next runs' non-spin
   CS work (insert) takes ~940 instead of ~440 TSC cycles and tokio-mutex w8
   sus drops to 0.19 Mops/s. 15 s idle removes it, 5 s does not; the matrix
   idles 20 s after each such run. Spins are TSC-timed, so frequency moves
   only non-spin work; earlier coro matrices had no blocking-lock runs.
   Cause not identified.
8. Initial placement differs: tokio takes every task from its injection
   queue; coro spawns client i on worker i mod W. Absolute numbers compare
   executors as well as locks; coro `dispatch` at 0.90–0.98× tokio-mutex is
   the executor anchor.

### Verdict

Of the off-the-shelf tokio locks only `tokio::sync::Mutex` serves every
client, and it lands where coro's FIFO mutex does (8 workers: 0.241 Mops/s
sustained, coro `dispatch` 0.90× / `dispatch-home` 1.05×; 0.073 bursty,
`dispatch` 0.97×), so the coro executor is not a weak baseline; the coop
budget never forced a yield, and lifting it moves throughput by 0–1 %.
Relative to tokio-mutex, `async-lock` is 0.73–0.76× in every cell because one
client takes the lock for the whole run in 11 of 12 runs (63 / 15 starved),
while `std-mutex` (w8: 0.85× sustained, 5.02× bursty) and `parking-lot`
(0.58×, 4.90×) are really W pinned threads that starve every client beyond
the W holding a worker, and every bystander. Against this field
`ces-k64-home`, `fc-remote` and `fcpq-h16-home` run 1.58 / 1.61 / 2.37×
tokio-mutex sustained and 4.36 / 4.22 / 5.01× bursty at 8 workers (1.60–2.36×
and 4.85–5.44× at 16) with no starved task and FIFO-level service Jain
(0.653–0.678 against tokio-mutex's 0.663–0.672), except `fcpq-h16-home`
(0.930 / 0.933 sustained, 0.726 at w16 bursty; its ops ratio counts cheap
light ops, phase 3). They beat std-mutex 1.80–3.28× in
three cells (w8 / w16 sustained, where it serves 8 / 16 of 64 clients, and
w16 bursty, where it serves all 16) and lose to it only at w8 bursty, where
it runs just 8 of the 16 clients (`ces-k64-home` 0.87×, `fc-remote` 0.84×,
`fcpq-h16-home` inside its spread). The bursty 4–5× over tokio-mutex is
mostly tokio's LIFO slot rather than the lock: with the slot disabled
(unstable build) tokio-mutex reaches 0.263 / 0.248 Mops/s bursty at 8 / 16
workers (`ces-k64-home` 1.21 / 1.41× ahead, all three variants 1.2–1.6×) and
0.250 / 0.248 sustained (`ces-k64-home` / `fc-remote` 1.50–1.55×). What
survives either tokio configuration is a 1.5–1.6× sustained margin for
`ces-k64-home` / `fc-remote`, fcpq-h16-home's service fairness, and a bursty
margin of 1.2–1.6×, not 4–5×, once the handoff wake can leave the unlocker's
worker.

## 2026-09-28 — Phase 3: mitigations (H-D), wake placement, H-C revisited

### One-line verdicts

- **H-D for CES — holds with `ces-k64-home`:** bounding the inline chain to
  K = 64 handoffs *and* placing the chain-break handoff on the head waiter's
  home worker restores burden Jain 0.993–0.999 in every cell (8 and 16
  workers, sustained and bursty, balance 0 and 31), makes the combiner-worker
  bystander p99 equal to the non-combiner p99 (2.6–2.8 µs sustained, 44.7 /
  14.9 µs bursty at 8 / 16 workers, same bucket) with zero starved tasks, at
  0.98–0.99× `ces` throughput sustained and 0.95–0.99× bursty — i.e. still
  1.72–1.84× `dispatch` sustained and 4.5–6.1× `dispatch` bursty. The bound
  alone (default placement) ends starvation but not concentration at balance
  0 (burden stays 1/W: the next chain restarts on the same worker; combiner
  bystander p99 = 171 µs = one 64-handoff chain); the cycle budget T = 64×light
  CS gives chains of 12–17 and costs 6–8 % (`ces-t64000-home`), and with
  default placement it *hurts* in bursty/balance-0 (0.79× `ces`, burden
  0.185).
- **Placement:** `remote` removes the balance-0 one-worker convergence of
  FC-style locks completely (`fc-remote` / `fcpq-remote`: burden 0.98,
  throughput 0.388–0.390 Mops/s = the balanced level, 1.01× `ces`); `home`
  removes it only partly (burden 0.74 `fc-home`, 0.25 `fcpq-home`, 0.5–0.6 for
  most fcpq knobs sustained; 0.69–0.95 bursty) because the election is
  sticky: the ex-combiner's own clients are woken locally and win the next
  `try_lock`. For `dispatch`, `home` gives +15–21 % sustained and 2.0–2.8×
  (8 workers) / 2.4–4.5× (16 workers) bursty, but `dispatch-home` is still
  0.45–0.50× `ces` in bursty at 8 workers (0.47–0.70× at 16): **placement
  alone does not close dispatch's bursty gap**; the remainder is the
  scheduler round-trip of a queued handoff versus an inline resume.
- **H-C revisited — still refuted on fairness (best 0.935), holds on ops
  cost, fails on work cost:** the pass limit is the knob that matters.
  `fcpq-h16-home` reaches service Jain 0.932 [0.930, 0.933] at 8 workers and
  0.935 [0.933, 0.936] at 16 (sustained, balancing), light:heavy ops 3.65
  against the 6.4 needed; `fcpq-h16` 0.863 / 0.856, `fcpq-h8` 0.763, pass
  budget `t16000` 0.66 (no effect), rotate / credit / elect 0.69–0.71 (no
  effect). Ops throughput is 1.44× `fc`, but that counts cheap light ops:
  lock utilisation (critical-section cycles per second) drops from 0.860
  (`fc`) to 0.725 (`fcpq-h16-home`), −16 % of work. Bursty: 0.67–0.75, where
  the pass budget `fcpq-t16000-home` is the best knob (0.736–0.751).

### Setup

As phase 2 (same box, toolchain, workload, 3 repeats, 2 s window, repeats
outermost; the unrelated `fcpq-loom` process was still running). Matrix:
workers 8, heavy 8, contention {sustained, bursty}, balance {0, 31}, 26
variants = 312 runs (`results/p3-<variant>-w8-h8-b<B>-<sus|bur>-r<i>.json`)
plus a 16-worker confirmation of {dispatch, dispatch-home, ces, ces-k64-home,
fcpq, fcpq-h16, fcpq-h16-home} = 84 runs (`p3-*-w16-*`). Executor additions:
`wake_remote`, `wake_home` (inbox of the worker that last polled the task),
per-worker placement counters and inline-chain-length histogram; `fc`/`fcpq`
default `yield_after_combine = true` throughout. Variant grammar: `-k<K>` /
`-t<T>` CES chain bound (handoffs / cycles), `-h<H>` fc/fcpq pass limit,
`-t<T>` fcpq pass budget, `-rotate` / `-credit` / `-elect` fcpq knobs,
`-home` / `-remote` wake placement (dispatch: handoff wake; CES: chain-break
handoff only, inline resumes inside a chain stay inline; fc/fcpq: every
lock-issued wake). Regenerate tables with
`python3 scripts/summarize.py results 'p3-*.json'`.

### H-D: CES chain bounds, 8 workers, heavy 8

| cell | variant | burden Jain | combiner / non-combiner bystander p99 (µs) | starved bystanders | service Jain | throughput (Mops/s) | thr/ces | thr/dispatch | chain p50 / max |
|---|---|---|---|---|---|---|---|---|---|
| sus b0 | ces | 0.125 | 2 196 125 (censored) / 2.6 | 1 | 0.617 [0.574, 0.646] (5 starved clients) | 0.386 [0.360, 0.411] | 1.00 | 1.84 | 819 200 / 847 375 |
| sus b0 | ces-k64 | 0.125 | 171.3 [171.3, 178.7] / 2.7 | 0 | 0.652 | 0.377 [0.377, 0.379] | 0.98 | 1.80 | 64 / 64 |
| sus b0 | ces-k64-home | 0.996 [0.993, 0.997] | 2.6 [2.6, 2.8] / 2.6 [2.6, 2.8] | 0 | 0.660 | 0.378 [0.378, 0.382] | 0.98 | 1.81 | 64 / 64 |
| sus b0 | ces-t64000 | 0.125 | 39.1 / 2.6 | 0 | 0.651 | 0.361 [0.360, 0.363] | 0.94 | 1.73 | 12 / 20 |
| sus b0 | ces-t64000-home | 0.999 [0.999, 1.000] | 2.6 / 2.6 | 0 | 0.660 | 0.354 [0.354, 0.356] | 0.92 | 1.69 | 12 / 17 |
| sus b31 | ces | 0.125 | – (no bystander) / 3.0 | 0 | 0.651 | 0.385 [0.384, 0.387] | 1.00 | 1.77 | 819 200 / 845 297 |
| sus b31 | ces-k64 | 0.904 [0.897, 0.978] | 3.0 / 3.0 | 0 | 0.656 | 0.380 [0.379, 0.380] | 0.99 | 1.75 | 64 / 64 |
| sus b31 | ces-k64-home | 0.993 [0.992, 0.995] | 2.7 [2.7, 2.8] / 2.9 [2.8, 3.0] | 0 | 0.659 | 0.378 [0.377, 0.379] | 0.98 | 1.74 | 64 / 64 |
| sus b31 | ces-t64000 | 0.993 [0.989, 0.998] | 2.8 / 2.8 | 0 | 0.658 | 0.363 [0.362, 0.365] | 0.94 | 1.67 | 12 / 18 |
| sus b31 | ces-t64000-home | 1.000 [0.999, 1.000] | 2.6 / 2.6 | 0 | 0.660 | 0.355 [0.354, 0.355] | 0.92 | 1.63 | 12 / 17 |
| bur b0 | ces | 0.999 | 44.7 / 44.7 | 0 | 0.679 | 0.317 [0.317, 0.318] | 1.00 | 5.55 | 12 / 171 [94, 191] |
| bur b0 | ces-k64-home | 0.996 [0.993, 0.999] | 44.7 / 44.7 | 0 | 0.678 | 0.314 [0.313, 0.317] | 0.99 | 5.49 | 12 / 64 |
| bur b0 | ces-t64000 | 0.185 [0.125, 0.213] | 52.1 [50.3, 63.3] / 29.8 [14.9, 29.8] | 0 | 0.645 | 0.251 [0.251, 0.252] | 0.79 | 4.40 | 12 / 14 |
| bur b0 | ces-t64000-home | 1.000 [0.998, 1.000] | 29.8 / 29.8 | 0 | 0.668 | 0.300 [0.292, 0.301] | 0.95 | 5.25 | 9 / 14 |
| bur b31 | ces | 1.000 | 44.7 / 44.7 | 0 | 0.679 | 0.317 [0.316, 0.317] | 1.00 | 4.49 | 12 / 152 [126, 247] |
| bur b31 | ces-k64-home | 0.996 [0.995, 0.998] | 44.7 / 44.7 | 0 | 0.678 | 0.314 [0.314, 0.316] | 0.99 | 4.45 | 12 / 64 |
| bur b31 | ces-t64000-home | 1.000 | 29.8 / 29.8 | 0 | 0.662 | 0.306 [0.299, 0.306] | 0.97 | 4.34 | 10 / 14 |

Reading: in bursty cells `ces` chains already end naturally (p50 12), so
the bound only trims the tail (max 152–191 → 64) and costs ≤ 1 %. The budget
variant breaks every ~12 handoffs even where the queue would have carried
on, hence its 5–8 % cost; with default placement the freshly woken owner
lands in the ex-combiner's local queue behind its bystander and the chain
restarts there (burden 0.19 bursty b0) — home placement is what moves the
role.

### Placement, 8 workers, heavy 8

| cell | variant | burden Jain | combiner / non-combiner p99 (µs) | service Jain | throughput (Mops/s) | thr/ces | thr/dispatch | yields/op |
|---|---|---|---|---|---|---|---|---|
| sus b0 | dispatch | – | – / 5.8 | 0.650 | 0.209 [0.208, 0.210] | 0.54 | 1.00 | – |
| sus b0 | dispatch-home | – | – / 6.5 | 0.671 | 0.253 [0.251, 0.256] | 0.66 | 1.21 | – |
| sus b0 | fc | 0.125 | 268.1 [268.1, 283.0] / 0.1 | 0.649 | 0.234 [0.232, 0.235] | 0.61 | 1.12 | 1.10 |
| sus b0 | fc-home | 0.744 [0.725, 0.829] | 2.3 [2.2, 2.4] / 2.4 | 0.636 [0.635, 0.641] | 0.369 [0.367, 0.373] | 0.96 | 1.76 | 0.02 |
| sus b0 | fc-remote | 0.981 [0.967, 0.996] | 2.6 [2.6, 2.7] / 2.6 [2.6, 2.7] | 0.654 [0.654, 0.658] | 0.388 [0.382, 0.388] | 1.01 | 1.85 | 0.02 |
| sus b0 | fcpq | 0.125 | 283.0 [268.1, 283.0] / 0.1 | 0.649 | 0.233 [0.231, 0.234] | 0.60 | 1.11 | 1.10 |
| sus b0 | fcpq-home | 0.250 | 0.2 / 4.4 | 0.615 | 0.350 [0.350, 0.351] | 0.91 | 1.67 | 0.02 |
| sus b0 | fcpq-remote | 0.983 [0.980, 0.993] | 2.7 [2.6, 2.7] / 2.7 [2.6, 2.7] | 0.661 [0.661, 0.662] | 0.390 | 1.01 | 1.86 | 0.02 |
| sus b31 | dispatch | – | – / 0.8 | 0.659 | 0.217 | 0.56 | 1.00 | – |
| sus b31 | dispatch-home | – | – / 3.5 [3.5, 3.6] | 0.671 [0.669, 0.671] | 0.251 | 0.65 | 1.15 | – |
| sus b31 | fc | 0.999 | 1.3 [1.2, 1.3] / 1.3 | 0.655 [0.654, 0.658] | 0.392 [0.387, 0.393] | 1.02 | 1.80 | 0.02 |
| sus b31 | fc-home | 0.986 [0.975, 0.994] | 2.6 [2.4, 2.6] / 2.6 [2.6, 2.7] | 0.654 | 0.378 [0.377, 0.378] | 0.98 | 1.74 | 0.02 |
| sus b31 | fc-remote | 0.993 [0.975, 0.995] | 2.9 [2.9, 3.0] / 3.0 | 0.654 | 0.387 [0.386, 0.387] | 1.00 | 1.78 | 0.02 |
| bur b0 | dispatch | – | – / 18.6 | 0.652 [0.652, 0.658] | 0.057 | 0.18 | 1.00 | – |
| bur b0 | dispatch-home | – | – / 18.6 | 0.670 | 0.158 [0.144, 0.190] | 0.50 | 2.77 | – |
| bur b0 | fc | 0.125 | 268.1 / 0.1 | 0.650 [0.650, 0.653] | 0.059 | 0.19 | 1.03 | 1.10 |
| bur b0 | fc-home | 0.946 [0.946, 0.947] | 37.2 / 37.2 | 0.662 [0.661, 0.662] | 0.358 [0.357, 0.358] | 1.13 | 6.26 | 0.13 |
| bur b0 | fc-remote | 1.000 | 29.8 / 29.8 | 0.657 [0.656, 0.660] | 0.304 [0.301, 0.304] | 0.96 | 5.32 | 0.26 |
| bur b0 | fcpq-home | 0.686 [0.677, 0.695] | 37.2 / 37.2 [35.4, 37.2] | 0.674 [0.674, 0.675] | 0.363 [0.362, 0.364] | 1.14 | 6.35 | 0.13 |
| bur b31 | dispatch | – | – / 0.8 | 0.676 [0.675, 0.676] | 0.070 [0.070, 0.071] | 0.22 | 1.00 | – |
| bur b31 | dispatch-home | – | – / 6.7 [5.4, 8.4] | 0.670 [0.670, 0.672] | 0.144 [0.142, 0.144] | 0.45 | 2.04 | – |
| bur b31 | fc | 1.000 | 29.8 / 29.8 | 0.658 [0.657, 0.659] | 0.302 [0.300, 0.302] | 0.95 | 4.28 | 0.34 |
| bur b31 | fc-home | 1.000 | 26.1 [26.1, 27.0] / 27.0 [27.0, 27.9] | 0.659 [0.658, 0.663] | 0.346 [0.340, 0.346] | 1.09 | 4.90 | 0.15 |
| bur b31 | fcpq | 1.000 | 29.8 / 29.8 | 0.668 [0.668, 0.669] | 0.305 [0.305, 0.306] | 0.96 | 4.33 | 0.35 |
| bur b31 | fcpq-home | 1.000 | 29.8 / 29.8 | 0.687 [0.687, 0.688] | 0.365 [0.363, 0.365] | 1.15 | 5.18 | 0.14 |

Reading: `home` is a throughput win for FC-style locks in bursty mode
(+15–20 % over their default at balance 31, 1.09–1.15× `ces`) because served
waiters resume their parallel work on their own worker instead of queueing
behind each other on the combiner's; it also cuts the cooperative yields per
op from 0.34 to 0.14. In sustained mode it costs 4 % for `fc` and 13 % for
`fcpq` (0.430 → 0.372), where it also cancels the usage-ordering gain
(service Jain 0.706 → 0.649, light:heavy ops 1.36 → 0.96).
`dispatch-home` raises the non-combiner bystander p99 (0.8 → 3.5 µs
sustained, 0.8 → 6.7 µs bursty) because handoff wakes now land in other
workers' inboxes.

### H-C revisited: fcpq knobs, sustained, 8 workers, heavy 8

| balance | variant | service Jain | light:heavy ops | throughput (Mops/s) | thr/fc | lock utilisation (CS cycles/s ÷ TSC) | burden Jain |
|---|---|---|---|---|---|---|---|
| 31 | fc | 0.655 [0.654, 0.658] | 1.00 | 0.392 [0.387, 0.393] | 1.00 | 0.860 [0.855, 0.860] | 0.999 |
| 31 | fcpq | 0.706 [0.706, 0.714] | 1.36 [1.36, 1.40] | 0.430 [0.429, 0.431] | 1.10 | 0.838 [0.833, 0.839] | 0.999 |
| 31 | fcpq-h8 | 0.763 [0.763, 0.764] | 1.80 | 0.464 [0.463, 0.465] | 1.18 | 0.806 [0.804, 0.806] | 0.999 |
| 31 | fcpq-h8-home | 0.716 [0.715, 0.716] | 1.43 [1.42, 1.43] | 0.411 [0.411, 0.412] | 1.05 | – | 0.999 |
| 31 | fcpq-h16 | 0.863 [0.862, 0.863] | 2.73 [2.73, 2.74] | 0.547 [0.546, 0.547] | 1.40 | 0.792 | 0.999 |
| 31 | fcpq-h16-home | 0.932 [0.930, 0.933] | 3.65 [3.64, 3.65] | 0.564 [0.563, 0.571] | 1.44 | 0.725 [0.724, 0.730] | 1.000 |
| 31 | fcpq-t16000 | 0.661 [0.660, 0.663] | 1.04 | 0.379 [0.376, 0.380] | 0.97 | – | 0.999 |
| 31 | fcpq-t16000-home | 0.656 [0.655, 0.658] | 1.00 | 0.358 [0.354, 0.360] | 0.91 | – | 0.999 |
| 31 | fcpq-rotate | 0.708 [0.707, 0.710] | 1.37 | 0.430 [0.428, 0.430] | 1.10 | – | 0.998 |
| 31 | fcpq-credit | 0.693 [0.692, 0.694] | 1.31 | 0.425 [0.425, 0.426] | 1.08 | – | 0.999 |
| 31 | fcpq-elect | 0.707 [0.706, 0.707] | 1.37 [1.36, 1.37] | 0.430 [0.430, 0.431] | 1.10 | – | 0.999 |
| 0 | fcpq-h16-home | 0.922 [0.920, 0.922] | 3.50 | 0.563 [0.563, 0.569] | 2.41 (fc collapsed) | – | 0.500 |
| 0 | fcpq-h8-home | 0.696 [0.695, 0.696] | 1.29 [1.28, 1.29] | 0.398 [0.398, 0.399] | 1.70 | – | 0.591 [0.576, 0.592] |

16-worker confirmation (sustained, balance 31): `fcpq-h16-home` 0.935
[0.933, 0.936], L:H 3.57, 0.546 Mops/s (1.45× `ces`); `fcpq-h16` 0.856
[0.853, 0.857]; `fcpq` 0.710; bursty `fcpq-h16-home` 0.729 [0.727, 0.729]
(balance 31) / 0.754 (balance 0). `ces-k64-home` at 16 workers: burden 0.998–
0.999, combiner p99 = non-combiner p99 (2.6 µs sustained, 14.9 µs bursty),
0.95–0.99× `ces`. `dispatch-home` at 16 workers: 1.11–1.17× `dispatch`
sustained, 2.4–4.5× bursty, 0.47–0.70× `ces`.

Reading: with H = 64 ≥ number of waiters every pass admits everyone, so
usage ordering only permutes within a pass and the share is FIFO's (0.71).
With H < waiters the low-usage (light) clients fill each pass and heavy
clients wait until the 8-pass starvation clamp lifts them; the achievable
light:heavy ratio therefore scales with H × clamp (1.8 at H = 8, 2.7 at
H = 16, sub-linear because the clamp also fires sooner in wall time with
short passes). Cycle budgets, rotation, credit and max-usage election do not
change the admission set and therefore do not move the share. `home`
placement adds ~1 more light op per heavy op at H = 16 (3.65 vs 2.73) but
subtracts at H = 8 (1.43 vs 1.80); [INFERENCE] the inbox round trip changes
which clients are republished before the next pass opens. Reaching 6.4
needs H ≈ 32 with the current clamp, or a longer clamp — untested.

### Surprises

1. **A chain bound without a placement change does nothing for burden at
   balance 0** (`ces-k64`: 1/W) — the ex-combiner drains its queue and the
   next acquirer is again one of its own clients. Burden fairness needs the
   handoff to leave the worker; only then does the bound decide *how often*.
2. **`remote` beats `home` for FC-style locks at balance 0** (0.98 vs
   0.25–0.74 burden) — `home` makes the election sticky, whereas the injector
   makes it a fair race. In bursty mode `home` is nevertheless the faster
   option (waiters resume parallel work where their cache is).
3. **fcpq's ops/s gains are partly accounting**: every fcpq variant that
   favours light clients raises ops/s and lowers the lock's busy fraction
   (0.86 → 0.73); the lock does less work per second while looking faster.
4. **dispatch-home shows the executor round trip is the floor**: even with
   the owner delivered to its home worker's inbox, bursty dispatch reaches
   only 0.45–0.50× `ces` (8 workers) because the woken owner still waits for
   its worker's current poll (a 14.5 µs parallel-work slice) to end.
5. `ces` at balance 0 still traps 1–7 clients on the combiner's worker (5
   [1, 7] at 8 workers) despite round-robin spawning; the bound variants
   trap none.

### Next question

Whether `ces-k64-home`'s numbers hold when bystanders are intermittently
runnable (idle gaps make steal-when-idle real and would let the plain `ces`
chain be interrupted by stealing), and where the K sweet spot lies between
the chain lengths that occur naturally in bursty mode (p50 12) and the
sustained tail (max 850 k): a sweep K ∈ {8, 16, 32, 64, 256} × placement
`home` would give the burden/throughput curve. For H-C: sweep the fcpq pass
limit H ∈ {24, 32, 48} and the starvation clamp {8, 16, 32} passes with
`home` placement, reporting Jain, light:heavy ops and lock utilisation
together.

### Full 8-worker tables (26 variants, from `scripts/summarize.py`)

### sus, workers=8, heavy=8, balance=0

| variant | burden Jain | combiner share | combiner bystander p99 | non-combiner p99 | starved bystanders | starved clients | service Jain | light/heavy ops | throughput (Mops/s) | thr/ces | thr/dispatch | chains p50 / max | yields/op |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| dispatch | – | – | – | 5.8 [5.8, 5.8] | 0 | 0 | 0.650 [0.649, 0.652] | 1.00 [1.00, 1.00] | 0.209 [0.208, 0.210] | 0.54 | 1.00 | 0 / 0 | 0.00 |
| dispatch-home | – | – | – | 6.5 [6.5, 6.5] | 0 | 0 | 0.671 [0.670, 0.671] | 1.00 [1.00, 1.00] | 0.253 [0.251, 0.256] | 0.66 | 1.21 | 0 / 0 | 0.00 |
| ces | 0.125 | 1.00 | 2196124.6 [2196123.7, 2196132.9] | 2.6 [2.6, 2.7] | 1 | 5 [1, 7] | 0.617 [0.574, 0.646] | 0.97 [0.78, 1.19] | 0.386 [0.360, 0.411] | 1.00 | 1.84 | 819200 [786432, 884736] / 847375 [791130, 902212] | 0.00 |
| ces-home | 0.125 | 1.00 | 2196118.8 [2196115.8, 2196135.5] | 2.7 [2.7, 2.8] | 1 | 0 [0, 4] | 0.651 [0.631, 0.652] | 1.00 [0.88, 1.00] | 0.387 [0.375, 0.387] | 1.00 | 1.85 | 819200 / 848918 [823291, 850239] | 0.00 |
| ces-k64 | 0.125 | 1.00 | 171.3 [171.3, 178.7] | 2.7 [2.7, 2.7] | 0 | 0 | 0.652 [0.651, 0.652] | 1.00 [1.00, 1.00] | 0.377 [0.377, 0.379] | 0.98 | 1.80 | 64 / 64 | 0.00 |
| ces-k64-home | 0.996 [0.993, 0.997] | 0.13 [0.13, 0.13] | 2.6 [2.6, 2.8] | 2.6 [2.6, 2.8] | 0 | 0 | 0.660 [0.659, 0.660] | 1.00 [1.00, 1.00] | 0.378 [0.378, 0.382] | 0.98 | 1.81 | 64 / 64 | 0.00 |
| ces-t64000 | 0.125 [0.125, 0.125] | 1.00 [1.00, 1.00] | 39.1 [39.1, 39.1] | 2.6 [2.6, 2.6] | 0 | 0 | 0.651 [0.650, 0.651] | 1.00 [1.00, 1.00] | 0.361 [0.360, 0.363] | 0.94 | 1.73 | 12 / 20 [20, 21] | 0.00 |
| ces-t64000-home | 0.999 [0.999, 1.000] | 0.13 [0.13, 0.13] | 2.6 [2.6, 2.6] | 2.6 [2.6, 2.6] | 0 | 0 | 0.660 [0.660, 0.661] | 1.00 [1.00, 1.00] | 0.354 [0.354, 0.356] | 0.92 | 1.69 | 12 / 17 | 0.00 |
| fc | 0.125 | 1.00 | 268.1 [268.1, 283.0] | 0.1 [0.1, 0.1] | 0 | 0 | 0.649 [0.648, 0.652] | 1.00 [1.00, 1.00] | 0.234 [0.232, 0.235] | 0.61 | 1.12 | 0 / 0 | 1.10 [1.10, 1.10] |
| fc-home | 0.744 [0.725, 0.829] | 0.24 [0.23, 0.26] | 2.3 [2.2, 2.4] | 2.4 [2.4, 2.4] | 0 | 0 | 0.636 [0.635, 0.641] | 0.92 [0.90, 0.93] | 0.369 [0.367, 0.373] | 0.96 | 1.76 | 0 / 0 | 0.02 [0.02, 0.02] |
| fc-remote | 0.981 [0.967, 0.996] | 0.16 [0.14, 0.16] | 2.6 [2.6, 2.7] | 2.6 [2.6, 2.7] | 0 | 0 | 0.654 [0.654, 0.658] | 1.00 [1.00, 1.00] | 0.388 [0.382, 0.388] | 1.01 | 1.85 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq | 0.125 | 1.00 | 283.0 [268.1, 283.0] | 0.1 [0.1, 0.2] | 0 | 0 | 0.649 [0.648, 0.650] | 1.00 [1.00, 1.00] | 0.233 [0.231, 0.234] | 0.60 | 1.11 | 0 / 0 | 1.10 [1.10, 1.10] |
| fcpq-credit | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.2 [0.1, 0.2] | 0 | 0 | 0.649 [0.648, 0.649] | 1.00 [1.00, 1.00] | 0.231 [0.231, 0.234] | 0.60 | 1.10 | 0 / 0 | 1.10 [1.10, 1.10] |
| fcpq-credit-home | 0.248 [0.248, 0.248] | 0.54 [0.54, 0.54] | 0.2 [0.2, 0.2] | 2.6 [2.6, 2.7] | 0 | 0 | 0.615 [0.614, 0.615] | 1.00 [1.00, 1.00] | 0.380 [0.380, 0.380] | 0.99 | 1.82 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-elect | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.2] | 0 | 0 | 0.648 [0.648, 0.649] | 1.00 [1.00, 1.00] | 0.234 [0.231, 0.234] | 0.61 | 1.11 | 0 / 0 | 1.10 [1.10, 1.10] |
| fcpq-elect-home | 0.500 [0.500, 0.500] | 0.25 [0.25, 0.25] | 2.3 [2.3, 2.3] | 2.6 [2.4, 2.7] | 0 | 0 | 0.701 [0.700, 0.701] | 1.33 [1.33, 1.33] | 0.418 [0.417, 0.418] | 1.08 | 1.99 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-h16 | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.1] | 0 | 0 | 0.649 [0.649, 0.649] | 1.00 [1.00, 1.00] | 0.234 [0.234, 0.234] | 0.61 | 1.12 | 0 / 0 | 1.10 [1.10, 1.10] |
| fcpq-h16-home | 0.500 [0.500, 0.500] | 0.25 [0.25, 0.26] | 7.2 [7.2, 9.3] | 7.2 [7.2, 12.1] | 0 | 0 | 0.922 [0.920, 0.922] | 3.50 [3.50, 3.50] | 0.563 [0.563, 0.569] | 1.46 | 2.69 | 0 / 0 | 0.03 [0.03, 0.03] |
| fcpq-h8 | 0.125 | 1.00 | 268.1 [268.1, 327.7] | 0.1 [0.1, 0.2] | 0 | 0 | 0.650 [0.649, 0.658] | 1.00 [1.00, 1.00] | 0.233 [0.227, 0.234] | 0.60 | 1.11 | 0 / 0 | 1.10 [1.10, 1.10] |
| fcpq-h8-home | 0.591 [0.576, 0.592] | 0.23 [0.23, 0.23] | 2.4 [2.4, 2.4] | 2.4 [2.4, 2.6] | 0 | 0 | 0.696 [0.695, 0.696] | 1.29 [1.28, 1.29] | 0.398 [0.398, 0.399] | 1.03 | 1.90 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-home | 0.250 [0.250, 0.250] | 0.50 [0.50, 0.50] | 0.2 [0.2, 0.2] | 4.4 [4.4, 4.4] | 0 | 0 | 0.615 [0.615, 0.615] | 0.75 [0.75, 0.75] | 0.350 [0.350, 0.351] | 0.91 | 1.67 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-remote | 0.983 [0.980, 0.993] | 0.16 [0.15, 0.16] | 2.7 [2.6, 2.7] | 2.7 [2.6, 2.7] | 0 | 0 | 0.661 [0.661, 0.662] | 1.04 [1.04, 1.05] | 0.390 [0.390, 0.390] | 1.01 | 1.86 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-rotate | 0.125 | 1.00 | 268.1 [268.1, 283.0] | 0.1 [0.1, 0.2] | 0 | 0 | 0.649 [0.648, 0.652] | 1.00 [1.00, 1.00] | 0.234 [0.228, 0.234] | 0.61 | 1.12 | 0 / 0 | 1.10 [1.10, 1.10] |
| fcpq-rotate-home | 0.250 [0.250, 0.250] | 0.50 [0.50, 0.50] | 0.2 [0.2, 0.2] | 4.4 [4.2, 4.4] | 0 | 0 | 0.615 [0.615, 0.615] | 0.75 [0.75, 0.75] | 0.351 [0.349, 0.351] | 0.91 | 1.67 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-t16000 | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.1] | 0 | 0 | 0.649 [0.648, 0.649] | 1.00 [1.00, 1.00] | 0.234 [0.234, 0.235] | 0.61 | 1.12 | 0 / 0 | 1.10 [1.10, 1.10] |
| fcpq-t16000-home | 0.776 [0.749, 0.827] | 0.20 [0.19, 0.21] | 2.4 [2.4, 2.4] | 2.4 [2.4, 2.4] | 0 | 0 | 0.640 [0.640, 0.641] | 0.91 [0.89, 0.92] | 0.350 [0.344, 0.354] | 0.91 | 1.67 | 0 / 0 | 0.02 [0.02, 0.02] |

### sus, workers=8, heavy=8, balance=31

| variant | burden Jain | combiner share | combiner bystander p99 | non-combiner p99 | starved bystanders | starved clients | service Jain | light/heavy ops | throughput (Mops/s) | thr/ces | thr/dispatch | chains p50 / max | yields/op |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| dispatch | – | – | – | 0.8 [0.8, 0.8] | 0 | 0 | 0.659 [0.659, 0.660] | 1.00 [1.00, 1.00] | 0.217 [0.217, 0.217] | 0.56 | 1.00 | 0 / 0 | 0.00 |
| dispatch-home | – | – | – | 3.5 [3.5, 3.6] | 0 | 0 | 0.671 [0.669, 0.671] | 1.00 [1.00, 1.00] | 0.251 [0.251, 0.251] | 0.65 | 1.15 | 0 / 0 | 0.00 |
| ces | 0.125 | 1.00 | – | 3.0 [3.0, 3.0] | 0 | 0 | 0.651 [0.651, 0.652] | 1.00 [1.00, 1.00] | 0.385 [0.384, 0.387] | 1.00 | 1.77 | 819200 / 845297 [843013, 848886] | 0.00 |
| ces-home | 0.125 | 1.00 | – | 3.1 [3.0, 3.1] | 0 | 0 | 0.651 [0.651, 0.652] | 1.00 [1.00, 1.00] | 0.387 [0.386, 0.387] | 1.00 | 1.78 | 819200 [0, 819200] / 847866 [0, 849587] | 0.00 |
| ces-k64 | 0.904 [0.897, 0.978] | 0.19 [0.15, 0.23] | 3.0 [3.0, 3.0] | 3.0 [3.0, 3.0] | 0 | 0 | 0.656 [0.656, 0.657] | 1.00 [1.00, 1.00] | 0.380 [0.379, 0.380] | 0.99 | 1.75 | 64 / 64 | 0.00 |
| ces-k64-home | 0.993 [0.992, 0.995] | 0.13 [0.13, 0.14] | 2.7 [2.7, 2.8] | 2.9 [2.8, 3.0] | 0 | 0 | 0.659 [0.659, 0.660] | 1.00 [1.00, 1.00] | 0.378 [0.377, 0.379] | 0.98 | 1.74 | 64 / 64 | 0.00 |
| ces-t64000 | 0.993 [0.989, 0.998] | 0.14 [0.14, 0.15] | 2.8 [2.8, 2.8] | 2.8 [2.8, 2.8] | 0 | 0 | 0.658 [0.658, 0.658] | 1.00 [1.00, 1.00] | 0.363 [0.362, 0.365] | 0.94 | 1.67 | 12 / 18 [18, 19] | 0.00 |
| ces-t64000-home | 1.000 [0.999, 1.000] | 0.13 [0.13, 0.13] | 2.6 [2.6, 2.6] | 2.6 [2.6, 2.6] | 0 | 0 | 0.660 [0.660, 0.661] | 1.00 [1.00, 1.00] | 0.355 [0.354, 0.355] | 0.92 | 1.63 | 12 / 17 | 0.00 |
| fc | 0.999 [0.998, 0.999] | 0.13 [0.13, 0.13] | 1.3 [1.2, 1.3] | 1.3 [1.3, 1.3] | 0 | 0 | 0.655 [0.654, 0.658] | 1.00 [1.00, 1.00] | 0.392 [0.387, 0.393] | 1.02 | 1.80 | 0 / 0 | 0.02 [0.02, 0.02] |
| fc-home | 0.986 [0.975, 0.994] | 0.14 [0.14, 0.16] | 2.6 [2.4, 2.6] | 2.6 [2.6, 2.7] | 0 | 0 | 0.654 [0.654, 0.655] | 1.00 [1.00, 1.00] | 0.378 [0.377, 0.378] | 0.98 | 1.74 | 0 / 0 | 0.02 [0.02, 0.02] |
| fc-remote | 0.993 [0.975, 0.995] | 0.14 [0.14, 0.16] | 2.9 [2.9, 3.0] | 3.0 [3.0, 3.0] | 0 | 0 | 0.654 [0.654, 0.654] | 1.00 [1.00, 1.00] | 0.387 [0.386, 0.387] | 1.00 | 1.78 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq | 0.999 [0.999, 0.999] | 0.13 [0.13, 0.13] | 1.3 [1.3, 1.6] | 1.4 [1.4, 1.6] | 0 | 0 | 0.706 [0.706, 0.714] | 1.36 [1.36, 1.40] | 0.430 [0.429, 0.431] | 1.12 | 1.98 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-credit | 0.999 [0.999, 0.999] | 0.13 [0.13, 0.13] | 1.3 [1.3, 1.3] | 1.3 [1.3, 1.4] | 0 | 0 | 0.693 [0.692, 0.694] | 1.31 [1.31, 1.31] | 0.425 [0.425, 0.426] | 1.10 | 1.96 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-credit-home | 0.990 [0.988, 0.993] | 0.14 [0.14, 0.15] | 2.6 [2.6, 2.7] | 2.8 [2.7, 2.8] | 0 | 0 | 0.649 [0.648, 0.650] | 0.99 [0.96, 1.02] | 0.375 [0.372, 0.378] | 0.97 | 1.72 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-elect | 0.999 [0.998, 0.999] | 0.13 [0.13, 0.13] | 1.3 [1.3, 1.3] | 1.3 [1.3, 1.4] | 0 | 0 | 0.707 [0.706, 0.707] | 1.37 [1.36, 1.37] | 0.430 [0.430, 0.431] | 1.12 | 1.98 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-elect-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 2.7 [2.7, 2.8] | 2.7 [2.7, 2.8] | 0 | 0 | 0.657 [0.657, 0.657] | 1.01 [1.01, 1.01] | 0.378 [0.378, 0.379] | 0.98 | 1.74 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-h16 | 0.999 [0.998, 0.999] | 0.13 [0.13, 0.14] | 2.6 [2.6, 2.7] | 2.6 [2.6, 2.6] | 0 | 0 | 0.863 [0.862, 0.863] | 2.73 [2.73, 2.74] | 0.547 [0.546, 0.547] | 1.42 | 2.52 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-h16-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 2.9 [2.9, 2.9] | 2.9 [2.9, 2.9] | 0 | 0 | 0.932 [0.930, 0.933] | 3.65 [3.64, 3.65] | 0.564 [0.563, 0.571] | 1.46 | 2.60 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-h8 | 0.999 [0.999, 1.000] | 0.13 [0.13, 0.14] | 1.5 [1.5, 1.5] | 1.6 [1.5, 1.6] | 0 | 0 | 0.763 [0.763, 0.764] | 1.80 [1.80, 1.80] | 0.464 [0.463, 0.465] | 1.20 | 2.13 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-h8-home | 0.999 [0.999, 1.000] | 0.13 [0.13, 0.13] | 2.8 [2.8, 2.8] | 2.8 [2.8, 2.8] | 0 | 0 | 0.716 [0.715, 0.716] | 1.43 [1.42, 1.43] | 0.411 [0.411, 0.412] | 1.07 | 1.89 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-home | 0.991 [0.978, 0.994] | 0.14 [0.14, 0.15] | 2.6 [2.6, 2.6] | 2.7 [2.7, 2.7] | 0 | 0 | 0.649 [0.648, 0.649] | 0.96 [0.96, 0.96] | 0.372 [0.371, 0.372] | 0.96 | 1.71 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-remote | 0.974 [0.941, 0.979] | 0.16 [0.16, 0.18] | 3.0 [3.0, 3.0] | 3.0 [3.0, 3.0] | 0 | 0 | 0.659 [0.659, 0.660] | 1.03 [1.03, 1.03] | 0.387 [0.387, 0.387] | 1.01 | 1.78 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-rotate | 0.998 [0.997, 0.999] | 0.13 [0.13, 0.13] | 1.3 [1.3, 1.4] | 1.4 [1.4, 1.4] | 0 | 0 | 0.708 [0.707, 0.710] | 1.37 [1.37, 1.37] | 0.430 [0.428, 0.430] | 1.12 | 1.98 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-rotate-home | 0.975 [0.967, 0.983] | 0.16 [0.15, 0.16] | 2.6 [2.6, 2.6] | 2.7 [2.7, 2.8] | 0 | 0 | 0.649 [0.649, 0.649] | 0.96 [0.96, 0.96] | 0.371 [0.371, 0.371] | 0.96 | 1.71 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-t16000 | 0.999 [0.998, 0.999] | 0.13 [0.13, 0.13] | 1.2 [1.2, 1.3] | 1.3 [1.2, 1.3] | 0 | 0 | 0.661 [0.660, 0.663] | 1.04 [1.04, 1.04] | 0.379 [0.376, 0.380] | 0.98 | 1.74 | 0 / 0 | 0.02 [0.02, 0.02] |
| fcpq-t16000-home | 0.999 [0.998, 0.999] | 0.13 [0.13, 0.13] | 2.7 [2.6, 2.8] | 2.8 [2.6, 2.9] | 0 | 0 | 0.656 [0.655, 0.658] | 1.00 [1.00, 1.00] | 0.358 [0.354, 0.360] | 0.93 | 1.65 | 0 / 0 | 0.03 [0.03, 0.03] |

### bur, workers=8, heavy=8, balance=0

| variant | burden Jain | combiner share | combiner bystander p99 | non-combiner p99 | starved bystanders | starved clients | service Jain | light/heavy ops | throughput (Mops/s) | thr/ces | thr/dispatch | chains p50 / max | yields/op |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| dispatch | – | – | – | 18.6 [18.6, 18.6] | 0 | 0 | 0.652 [0.652, 0.658] | 1.00 [1.00, 1.00] | 0.057 [0.057, 0.057] | 0.18 | 1.00 | 0 / 0 | 0.00 |
| dispatch-home | – | – | – | 18.6 [18.6, 18.6] | 0 | 0 | 0.670 [0.670, 0.671] | 1.00 [1.00, 1.00] | 0.158 [0.144, 0.190] | 0.50 | 2.77 | 0 / 0 | 0.00 |
| ces | 0.999 [0.999, 1.000] | 0.13 [0.13, 0.13] | 44.7 [44.7, 44.7] | 44.7 [44.7, 44.7] | 0 | 0 | 0.679 [0.679, 0.679] | 1.13 [1.13, 1.13] | 0.317 [0.317, 0.318] | 1.00 | 5.55 | 12 [11, 12] / 171 [94, 191] | 0.00 |
| ces-home | 0.999 [0.999, 0.999] | 0.13 [0.13, 0.13] | 44.7 [44.7, 44.7] | 44.7 [44.7, 44.7] | 0 | 0 | 0.679 [0.679, 0.679] | 1.13 [1.13, 1.13] | 0.317 [0.317, 0.317] | 1.00 | 5.54 | 12 [11, 12] / 154 [121, 173] | 0.00 |
| ces-k64 | 0.999 [0.999, 1.000] | 0.13 [0.13, 0.13] | 44.7 [44.7, 44.7] | 44.7 [44.7, 44.7] | 0 | 0 | 0.678 [0.677, 0.678] | 1.12 [1.12, 1.13] | 0.317 [0.316, 0.317] | 1.00 | 5.54 | 12 / 64 | 0.00 |
| ces-k64-home | 0.996 [0.993, 0.999] | 0.13 [0.13, 0.13] | 44.7 [44.7, 44.7] | 44.7 [44.7, 44.7] | 0 | 0 | 0.678 [0.678, 0.679] | 1.13 [1.12, 1.13] | 0.314 [0.313, 0.317] | 0.99 | 5.49 | 12 [11, 12] / 64 | 0.00 |
| ces-t64000 | 0.185 [0.125, 0.213] | 0.80 [0.72, 1.00] | 52.1 [50.3, 63.3] | 29.8 [14.9, 29.8] | 0 | 0 | 0.645 [0.644, 0.646] | 0.95 [0.95, 0.95] | 0.251 [0.251, 0.252] | 0.79 | 4.40 | 12 / 14 | 0.00 |
| ces-t64000-home | 1.000 [0.998, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.668 [0.666, 0.668] | 1.05 [1.03, 1.05] | 0.300 [0.292, 0.301] | 0.95 | 5.25 | 9 [9, 10] / 14 | 0.00 |
| fc | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.1] | 0 | 0 | 0.650 [0.650, 0.653] | 1.00 [1.00, 1.00] | 0.059 [0.059, 0.059] | 0.19 | 1.03 | 0 / 0 | 1.10 [1.10, 1.11] |
| fc-home | 0.946 [0.946, 0.947] | 0.16 [0.16, 0.16] | 37.2 [37.2, 37.2] | 37.2 [37.2, 37.2] | 0 | 0 | 0.662 [0.661, 0.662] | 1.04 [1.04, 1.04] | 0.358 [0.357, 0.358] | 1.13 | 6.26 | 0 / 0 | 0.13 [0.13, 0.13] |
| fc-remote | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.657 [0.656, 0.660] | 1.00 [1.00, 1.00] | 0.304 [0.301, 0.304] | 0.96 | 5.32 | 0 / 0 | 0.26 [0.26, 0.26] |
| fcpq | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.2] | 0 | 0 | 0.651 [0.650, 0.653] | 1.00 [1.00, 1.00] | 0.059 [0.059, 0.059] | 0.18 | 1.03 | 0 / 0 | 1.10 [1.10, 1.10] |
| fcpq-credit | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.2] | 0 | 0 | 0.651 [0.651, 0.651] | 1.00 [1.00, 1.00] | 0.059 [0.058, 0.059] | 0.19 | 1.03 | 0 / 0 | 1.10 [1.10, 1.11] |
| fcpq-credit-home | 0.630 [0.630, 0.745] | 0.27 [0.20, 0.27] | 39.1 [39.1, 39.1] | 37.2 [37.2, 39.1] | 0 | 0 | 0.675 [0.672, 0.675] | 1.14 [1.14, 1.16] | 0.365 [0.364, 0.366] | 1.15 | 6.39 | 0 / 0 | 0.13 [0.13, 0.13] |
| fcpq-elect | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.1] | 0 | 0 | 0.651 [0.650, 0.651] | 1.00 [1.00, 1.00] | 0.059 [0.059, 0.059] | 0.19 | 1.03 | 0 / 0 | 1.10 [1.10, 1.11] |
| fcpq-elect-home | 0.948 [0.937, 0.949] | 0.16 [0.15, 0.16] | 37.2 [37.2, 37.2] | 37.2 [37.2, 37.2] | 0 | 0 | 0.689 [0.687, 0.690] | 1.22 [1.22, 1.23] | 0.360 [0.359, 0.361] | 1.14 | 6.30 | 0 / 0 | 0.12 [0.12, 0.12] |
| fcpq-h16 | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.1] | 0 | 0 | 0.651 [0.651, 0.652] | 1.00 [1.00, 1.00] | 0.059 [0.059, 0.059] | 0.19 | 1.03 | 0 / 0 | 1.10 [1.10, 1.11] |
| fcpq-h16-home | 0.696 [0.678, 0.721] | 0.29 [0.27, 0.29] | 37.2 [37.2, 37.2] | 35.4 [35.4, 37.2] | 0 | 0 | 0.674 [0.672, 0.674] | 1.13 [1.12, 1.13] | 0.363 [0.362, 0.364] | 1.14 | 6.35 | 0 / 0 | 0.13 [0.13, 0.13] |
| fcpq-h8 | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.1] | 0 | 0 | 0.651 [0.651, 0.651] | 1.00 [1.00, 1.00] | 0.059 [0.059, 0.059] | 0.19 | 1.03 | 0 / 0 | 1.10 [1.10, 1.11] |
| fcpq-h8-home | 0.891 [0.881, 0.901] | 0.21 [0.20, 0.21] | 39.1 [39.1, 42.8] | 39.1 [39.1, 39.1] | 0 | 0 | 0.679 [0.679, 0.679] | 1.16 [1.16, 1.16] | 0.365 [0.364, 0.365] | 1.15 | 6.39 | 0 / 0 | 0.14 [0.14, 0.14] |
| fcpq-home | 0.686 [0.677, 0.695] | 0.29 [0.29, 0.29] | 37.2 [37.2, 37.2] | 37.2 [35.4, 37.2] | 0 | 0 | 0.674 [0.674, 0.675] | 1.12 [1.12, 1.13] | 0.363 [0.362, 0.364] | 1.14 | 6.35 | 0 / 0 | 0.13 [0.13, 0.13] |
| fcpq-remote | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.671 [0.671, 0.671] | 1.09 [1.09, 1.09] | 0.308 [0.308, 0.309] | 0.97 | 5.40 | 0 / 0 | 0.26 [0.26, 0.27] |
| fcpq-rotate | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.1] | 0 | 0 | 0.651 [0.650, 0.653] | 1.00 [1.00, 1.00] | 0.059 [0.059, 0.059] | 0.19 | 1.03 | 0 / 0 | 1.10 [1.10, 1.10] |
| fcpq-rotate-home | 0.704 [0.680, 0.712] | 0.28 [0.28, 0.29] | 37.2 [37.2, 41.0] | 35.4 [35.4, 35.4] | 0 | 0 | 0.674 [0.673, 0.674] | 1.13 [1.13, 1.13] | 0.364 [0.363, 0.365] | 1.15 | 6.37 | 0 / 0 | 0.13 [0.13, 0.13] |
| fcpq-t16000 | 0.125 | 1.00 | 268.1 [268.1, 268.1] | 0.1 [0.1, 0.1] | 0 | 0 | 0.651 [0.650, 0.651] | 1.00 [1.00, 1.00] | 0.059 [0.059, 0.059] | 0.19 | 1.03 | 0 / 0 | 1.10 [1.10, 1.10] |
| fcpq-t16000-home | 0.890 [0.888, 0.953] | 0.17 [0.15, 0.17] | 29.8 [14.9, 29.8] | 41.0 [39.1, 41.0] | 0 | 0 | 0.751 [0.749, 0.785] | 1.68 [1.67, 1.91] | 0.378 [0.354, 0.378] | 1.19 | 6.62 | 0 / 0 | 0.14 [0.14, 0.15] |

### bur, workers=8, heavy=8, balance=31

| variant | burden Jain | combiner share | combiner bystander p99 | non-combiner p99 | starved bystanders | starved clients | service Jain | light/heavy ops | throughput (Mops/s) | thr/ces | thr/dispatch | chains p50 / max | yields/op |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| dispatch | – | – | – | 0.8 [0.8, 0.8] | 0 | 0 | 0.676 [0.675, 0.676] | 1.00 [1.00, 1.00] | 0.070 [0.070, 0.071] | 0.22 | 1.00 | 0 / 0 | 0.00 |
| dispatch-home | – | – | – | 6.7 [5.4, 8.4] | 0 | 0 | 0.670 [0.670, 0.672] | 1.00 [1.00, 1.00] | 0.144 [0.142, 0.144] | 0.45 | 2.04 | 0 / 0 | 0.00 |
| ces | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 44.7 [44.7, 44.7] | 44.7 [44.7, 44.7] | 0 | 0 | 0.679 [0.678, 0.679] | 1.13 [1.12, 1.13] | 0.317 [0.316, 0.317] | 1.00 | 4.49 | 12 / 152 [126, 247] | 0.00 |
| ces-home | 1.000 [0.972, 1.000] | 0.13 [0.13, 0.14] | 44.7 [44.7, 44.7] | 44.7 [44.7, 44.7] | 0 | 0 | 0.679 [0.679, 0.683] | 1.13 [1.13, 1.15] | 0.316 [0.312, 0.320] | 1.00 | 4.48 | 11 [9, 12] / 148 [128, 223] | 0.00 |
| ces-k64 | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 44.7 [44.7, 44.7] | 44.7 [44.7, 44.7] | 0 | 0 | 0.679 [0.678, 0.679] | 1.13 [1.13, 1.13] | 0.317 [0.317, 0.318] | 1.00 | 4.50 | 12 [11, 12] / 64 | 0.00 |
| ces-k64-home | 0.996 [0.995, 0.998] | 0.13 [0.13, 0.14] | 44.7 [44.7, 44.7] | 44.7 [44.7, 44.7] | 0 | 0 | 0.678 [0.678, 0.679] | 1.12 [1.12, 1.13] | 0.314 [0.314, 0.316] | 0.99 | 4.45 | 12 [11, 12] / 64 | 0.00 |
| ces-t64000 | 0.989 [0.973, 0.993] | 0.14 [0.14, 0.15] | 15.8 [15.8, 15.8] | 15.8 [15.8, 15.8] | 0 | 0 | 0.652 [0.652, 0.652] | 0.95 [0.95, 0.96] | 0.257 [0.256, 0.257] | 0.81 | 3.64 | 12 / 14 | 0.00 |
| ces-t64000-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.662 [0.661, 0.662] | 1.01 [1.00, 1.01] | 0.306 [0.299, 0.306] | 0.97 | 4.34 | 10 / 14 | 0.00 |
| fc | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.658 [0.657, 0.659] | 1.01 [1.00, 1.01] | 0.302 [0.300, 0.302] | 0.95 | 4.28 | 0 / 0 | 0.34 [0.34, 0.34] |
| fc-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 26.1 [26.1, 27.0] | 27.0 [27.0, 27.9] | 0 | 0 | 0.659 [0.658, 0.663] | 1.01 [1.01, 1.01] | 0.346 [0.340, 0.346] | 1.09 | 4.90 | 0 / 0 | 0.15 [0.14, 0.15] |
| fc-remote | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.657 [0.656, 0.657] | 1.00 [1.00, 1.00] | 0.307 [0.306, 0.307] | 0.97 | 4.35 | 0 / 0 | 0.25 [0.25, 0.25] |
| fcpq | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.668 [0.668, 0.669] | 1.07 [1.07, 1.08] | 0.305 [0.305, 0.306] | 0.96 | 4.33 | 0 / 0 | 0.35 [0.35, 0.35] |
| fcpq-credit | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.669 [0.668, 0.669] | 1.07 [1.07, 1.08] | 0.305 [0.302, 0.305] | 0.96 | 4.32 | 0 / 0 | 0.35 [0.35, 0.35] |
| fcpq-credit-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.686 [0.685, 0.686] | 1.21 [1.20, 1.21] | 0.368 [0.368, 0.368] | 1.16 | 5.23 | 0 / 0 | 0.14 [0.14, 0.14] |
| fcpq-elect | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.672 [0.672, 0.673] | 1.10 [1.10, 1.10] | 0.307 [0.306, 0.307] | 0.97 | 4.35 | 0 / 0 | 0.36 [0.36, 0.36] |
| fcpq-elect-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.689 [0.688, 0.689] | 1.22 [1.22, 1.22] | 0.363 [0.362, 0.363] | 1.14 | 5.14 | 0 / 0 | 0.14 [0.14, 0.14] |
| fcpq-h16 | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.669 [0.668, 0.669] | 1.08 [1.08, 1.08] | 0.304 [0.304, 0.305] | 0.96 | 4.32 | 0 / 0 | 0.35 [0.35, 0.35] |
| fcpq-h16-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.687 [0.685, 0.687] | 1.20 [1.20, 1.21] | 0.365 [0.365, 0.366] | 1.15 | 5.18 | 0 / 0 | 0.14 [0.14, 0.14] |
| fcpq-h8 | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.668 [0.668, 0.669] | 1.07 [1.07, 1.08] | 0.305 [0.305, 0.305] | 0.96 | 4.33 | 0 / 0 | 0.35 [0.35, 0.35] |
| fcpq-h8-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.682 [0.681, 0.683] | 1.17 [1.17, 1.17] | 0.359 [0.358, 0.359] | 1.13 | 5.09 | 0 / 0 | 0.15 [0.15, 0.15] |
| fcpq-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.687 [0.687, 0.688] | 1.21 [1.21, 1.21] | 0.365 [0.363, 0.365] | 1.15 | 5.18 | 0 / 0 | 0.14 [0.14, 0.14] |
| fcpq-remote | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 31.6] | 31.6 [29.8, 33.5] | 0 | 0 | 0.670 [0.670, 0.671] | 1.09 [1.08, 1.09] | 0.310 [0.309, 0.310] | 0.98 | 4.39 | 0 / 0 | 0.26 [0.26, 0.27] |
| fcpq-rotate | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.669 [0.668, 0.671] | 1.08 [1.08, 1.08] | 0.302 [0.300, 0.305] | 0.95 | 4.28 | 0 / 0 | 0.36 [0.35, 0.36] |
| fcpq-rotate-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.686 [0.686, 0.687] | 1.21 [1.20, 1.21] | 0.366 [0.365, 0.366] | 1.15 | 5.18 | 0 / 0 | 0.14 [0.14, 0.14] |
| fcpq-t16000 | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 29.8] | 29.8 [29.8, 29.8] | 0 | 0 | 0.672 [0.671, 0.675] | 1.09 [1.09, 1.10] | 0.306 [0.303, 0.307] | 0.97 | 4.34 | 0 / 0 | 0.36 [0.36, 0.36] |
| fcpq-t16000-home | 1.000 [1.000, 1.000] | 0.13 [0.13, 0.13] | 29.8 [29.8, 31.6] | 31.6 [31.6, 31.6] | 0 | 0 | 0.736 [0.735, 0.743] | 1.56 [1.56, 1.58] | 0.386 [0.384, 0.387] | 1.22 | 5.48 | 0 / 0 | 0.16 [0.16, 0.16] |


## 2026-09-28 — Phase 2 matrix: H-A, H-B (bursty), H-C

### One-line verdicts

- **H-A (combiner burden concentrates, burden Jain < 0.7 at 8 workers):**
  **holds for CES** in every saturated sustained cell, in the extreme form
  burden Jain = 1/W (0.125 at 8 workers, 0.062 at 16): one worker runs 100 %
  of all critical sections for the whole 2 s window, with or without executor
  balancing. **Refuted for FC and FC-PQ** under balancing (burden Jain
  0.996–1.000: the `try_lock` election hands the combiner role to whichever
  worker polls first, which is uniformly spread); without balancing FC-style
  locks also show 1/W, but because *all client tasks* converge onto the
  combiner's worker (executor placement), not because of the election.
- **H-B (bursty; combiner-worker bystander p99 exceeds non-combiner p99 by
  more than one pass H×mean CS):** **refuted under balancing** in all 12
  bursty cells for all five combining variants: the combiner role rotates and
  the two p99s are equal to within one histogram bucket (e.g. 8 workers,
  heavy 8: CES 44.7 vs 44.7 µs, FC 29.8 vs 29.8 µs, against a pass of
  137–142 µs). **Without balancing the F2 effect is starvation, not delay:**
  CES at 16 workers/heavy 8 keeps one chain alive through the whole window and
  its bystander is polled once, after 2.2 s (censored sample); FC/FC-PQ show a
  268 µs difference (> pass 139 µs) but that is the bystander waiting behind
  the 16 converged clients on the same worker, not behind a pass.
- **H-C (usage-ordered selection gives service Jain ≥ 0.95 where FIFO ≤ 0.90,
  at ≥ 0.9× FIFO throughput):** **refuted on fairness, holds on cost.**
  FC-PQ reaches service Jain 0.713–0.720 (sustained, heavy 8, balancing)
  versus 0.650–0.662 for dispatch/CES/FC; light clients get 1.35–1.45× the ops
  of heavy clients where equal service would need 6.4×. Throughput is *higher*
  than FC by 8–12 % at heavy 8 (more light ops per second) and within
  0–3 % of FC at heavy 1 (tax ≤ 3 %, i.e. below the 10 % threshold).

### Setup

- Box: 2 × Intel Xeon Gold 6438M (32 physical cores per socket, SMT on, 128
  logical), Linux 6.17.7, TSC 2.20 GHz (`rdtscp`), rustc 1.100.0-nightly
  (6bb1652a0 2026-09-22), `--release` (workspace profile: `debug = true`).
  Workers pinned one per physical core, logical CPUs 0..W−1 (SMT siblings are
  i and i+64). Commit id not recorded (VCS commands are off-limits in this
  workspace; Main records it at commit time).
- External load: one unrelated single-threaded process (`fcpq-loom` from
  `.worktree/hps-audit`, unpinned, last seen on CPU 62) ran during the whole
  matrix. Its effect, if any, is inside the reported spreads.
- Workload (RESEARCH.md): `BTreeMap<u64,u64>` insert + `spin_cycles`, light
  CS 1000 cycles, heavy = ratio × light, key space 65 536; bystanders =
  workers, 1000 cycles per poll then `yield_now`; 200 ms warm-up, 2 s window,
  3 repeats, sequential, repeats as the outermost loop.
- Matrix: locks {dispatch, ces, fc, fcpq, fc-noyield, fcpq-noyield} × workers
  {4, 8, 16} × heavy ratio {1, 8} × balance interval {0, 31} × contention
  {sustained: 64 clients, parallel work 4× light; bursty: 16 clients, 32×}
  = 432 runs, `results/matrix-<lock>-w<W>-h<H>-b<B>-<sus|bur>-r<i>.json`.
  `fc`/`fcpq` are the locks' current default (`yield_after_combine = true`,
  added by Combiners on 2026-09-28 after the collapse described under
  Surprises); `*-noyield` is the original flat-combining behaviour
  (`--no-combiner-yield`).
- Executor knobs (see `src/executor.rs` docs): injector checked every 31
  polls, draining up to len/W+1 items and polling each immediately; balancing
  steal every `balance_interval` polls (0 = off); clients and bystanders
  spawned round-robin onto workers (`spawn_on`).
- Metric definitions: *combiner worker* = worker with the largest
  `combining_cycles` in a run; *non-combiner p99* = max bystander p99 over
  the other workers; *pass* = 64 × mean CS cycles of the run; bystander
  latency is attributed to the worker whose queue the task was scheduled
  onto; a *censored* sample is a bystander polled for the first time during
  the drain (value ≈ window + warm-up, ≥ 2.07 s here) — such a task never ran
  during the window. `starved clients` = clients with 0 ops in the window.
  Latencies are histogram bucket lower bounds (log-linear, 6 % resolution).

### Key cells: 8 workers, heavy ratio 8

Sustained (64 clients, 4× parallel), balance 0:

| lock | throughput (Mops/s) | service Jain | burden Jain | combiner share | combiner bystander p99 (µs) | non-combiner p99 (µs) | pass (µs) | light run p50 / p99 (µs) | heavy run p50 / p99 (µs) | starved clients | starved bystanders |
|---|---|---|---|---|---|---|---|---|---|---|---|
| dispatch | 0.209 [0.199, 0.210] | 0.650 [0.650, 0.651] | – | – | – | 5.8 [5.8, 6.1] | 138.9 | 297.9 / 297.9 [297.9, 312.8] | 297.9 / 297.9 | 0 | 0 |
| ces | 0.404 [0.363, 0.413] | 0.574 [0.557, 0.627] | 0.125 | 1.00 | 2 196 125 (censored) | 2.8 [2.7, 2.8] | 130.7 [128.2, 148.1] | 141.5 [134.0, 156.4] / 156.4 [148.9, 171.3] | same | 5 [5, 6] | 1 |
| fc-noyield | 0.392 [0.391, 0.392] | 0.016 | 0.125 | 1.00 | 2 196 131 (censored) | 0.1 | 36.4 | 0.7 / 0.7 | 0.0 / 0.0 | 63 | 1 |
| fc | 0.235 [0.234, 0.235] | 0.649 | 0.125 | 1.00 | 268.1 | 0.1 [0.1, 0.2] | 138.4 | 268.1 / 268.1 | 268.1 / 268.1 | 0 | 0 |
| fcpq | 0.234 [0.231, 0.235] | 0.649 [0.649, 0.650] | 0.125 | 1.00 | 268.1 | 0.1 [0.1, 0.2] | 138.7 | 268.1 / 268.1 | 268.1 / 268.1 | 0 | 0 |
| fcpq-noyield | 0.175 [0.174, 0.175] | 0.016 | 0.125 | 1.00 | 2 196 123 (censored) | 0.1 | 240.2 | 0.0 / 0.0 | 3.7 / 3.7 [3.7, 4.0] | 63 | 1 |

Sustained, balance 31:

| lock | throughput (Mops/s) | service Jain | burden Jain | combiner share | combiner bystander p99 (µs) | non-combiner p99 (µs) | pass (µs) | light run p50 / p99 (µs) | heavy run p50 / p99 (µs) | starved clients | starved bystanders |
|---|---|---|---|---|---|---|---|---|---|---|---|
| dispatch | 0.216 [0.208, 0.218] | 0.661 [0.661, 0.664] | – | – | – | 0.8 [0.7, 0.8] | 142.4 | 283.0 [283.0, 297.9] / 297.9 [297.9, 312.8] | same | 0 | 0 |
| ces | 0.380 [0.378, 0.384] | 0.651 [0.650, 0.652] | 0.125 | 1.00 | – (no bystander ever on it) | 3.0 [3.0, 3.1] | 139.3 | 163.8 [156.4, 163.8] / 178.7 | same | 0 | 0 |
| fc-noyield | 0.391 [0.390, 0.392] | 0.657 [0.657, 0.658] | 0.999 [0.998, 0.999] | 0.13 | 1.2 [1.2, 1.3] | 1.3 | 140.9 | 156.4 / 238.3 | same | 0 | 0 |
| fc | 0.390 [0.389, 0.392] | 0.658 | 0.999 | 0.13 [0.13, 0.14] | 1.3 | 1.3 | 141.2 | 156.4 / 238.3 | same | 0 | 0 |
| fcpq | 0.431 [0.429, 0.432] | 0.713 [0.713, 0.714] | 0.998 [0.997, 0.999] | 0.13 [0.13, 0.14] | 1.4 [1.3, 1.5] | 1.5 [1.4, 1.5] | 124.6 [124.6, 125.3] | 119.1 [119.1, 126.6] / 216.0 | 171.3 / 268.1 | 0 | 0 |
| fcpq-noyield | 0.430 [0.429, 0.432] | 0.712 [0.711, 0.713] | 0.998 [0.998, 1.000] | 0.13 [0.13, 0.14] | 1.3 | 1.3 [1.3, 1.4] | 125.1 | 126.6 [119.1, 126.6] / 216.0 | 171.3 [163.8, 171.3] / 268.1 | 0 | 0 |

Bursty (16 clients, 32× parallel), balance 0:

| lock | throughput (Mops/s) | service Jain | burden Jain | combiner share | combiner bystander p99 (µs) | non-combiner p99 (µs) | pass (µs) | light run p50 / p99 (µs) | starved clients | starved bystanders |
|---|---|---|---|---|---|---|---|---|---|---|
| dispatch | 0.057 [0.056, 0.057] | 0.652 | – | – | – | 18.6 | 139.3 | 253.2 [253.2, 268.1] / 268.1 [253.2, 268.1] | 0 | 0 |
| ces | 0.315 [0.312, 0.318] | 0.679 | 1.000 [0.999, 1.000] | 0.13 | 44.7 | 44.7 | 136.3 | 39.1 / 59.6 | 0 | 0 |
| fc-noyield | 0.054 | 0.062 | 0.125 | 1.00 | 2 196 123 (censored) | 0.1 | 240.7 | 0.0 / 0.0 | 15 | 1 |
| fc | 0.059 | 0.650 [0.650, 0.651] | 0.125 | 1.00 | 268.1 | 0.1 [0.1, 0.2] | 138.8 | 253.2 / 253.2 | 0 | 0 |
| fcpq | 0.059 [0.058, 0.059] | 0.651 [0.650, 0.651] | 0.125 | 1.00 | 268.1 | 0.1 [0.1, 0.2] | 139.1 | 253.2 / 253.2 | 0 | 0 |
| fcpq-noyield | 0.054 [0.054, 0.065] | 0.062 | 0.125 | 1.00 | 2 196 123 (censored) | 0.1 | 240.4 [37.1, 240.6] | 0.0 [0.0, 0.7] / 0.0 [0.0, 0.8] | 15 | 1 |

Bursty, balance 31:

| lock | throughput (Mops/s) | service Jain | burden Jain | combiner share | combiner bystander p99 (µs) | non-combiner p99 (µs) | pass (µs) | light run p50 / p99 (µs) | heavy run p50 / p99 (µs) | starved clients | starved bystanders |
|---|---|---|---|---|---|---|---|---|---|---|---|
| dispatch | 0.071 [0.070, 0.071] | 0.673 [0.672, 0.675] | – | – | – | 0.8 | 146.2 | 208.5 / 238.3 | same | 0 | 0 |
| ces | 0.316 [0.315, 0.317] | 0.678 [0.678, 0.680] | 1.000 [0.999, 1.000] | 0.13 | 44.7 | 44.7 | 136.6 | 39.1 / 59.6 | same | 0 | 0 |
| fc-noyield | 0.298 [0.296, 0.298] | 0.660 [0.659, 0.660] | 1.000 | 0.13 | 29.8 [28.9, 29.8] | 29.8 | 141.5 | 35.4 / 126.6 [119.1, 126.6] | 33.5 [33.5, 35.4] / 126.6 | 0 | 0 |
| fc | 0.302 [0.301, 0.302] | 0.660 [0.660, 0.661] | 1.000 | 0.13 | 29.8 | 29.8 | 141.5 | 33.5 [33.5, 35.4] / 96.8 | same | 0 | 0 |
| fcpq | 0.304 [0.304, 0.305] | 0.672 [0.671, 0.672] | 1.000 | 0.13 | 29.8 | 29.8 | 138.2 | 31.6 / 96.8 [93.1, 96.8] | 35.4 / 96.8 [96.8, 100.5] | 0 | 0 |
| fcpq-noyield | 0.302 [0.300, 0.302] | 0.674 [0.674, 0.675] | 1.000 | 0.13 | 29.8 | 29.8 | 137.1 | 31.6 / 119.1 | 35.4 / 134.0 | 0 | 0 |

### H-A: burden Jain, sustained, all cells

| workers | heavy | balance | ces | fc | fcpq | fc-noyield | fcpq-noyield |
|---|---|---|---|---|---|---|---|
| 4 | 1 | 0 | 0.998 [0.998, 0.999] | 0.250 | 0.250 [0.250, 0.354] | 0.250 | 0.250 |
| 4 | 1 | 31 | 0.999 [0.998, 0.999] | 1.000 | 1.000 | 1.000 | 1.000 |
| 4 | 8 | 0 | 0.250 | 0.250 | 0.250 | 0.250 | 0.250 |
| 4 | 8 | 31 | 0.250 | 1.000 | 1.000 | 1.000 | 1.000 |
| 8 | 1 | 0 | 0.125 | 0.125 | 0.125 | 0.125 | 0.125 |
| 8 | 1 | 31 | 0.125 | 1.000 | 1.000 [0.999, 1.000] | 1.000 | 1.000 [0.999, 1.000] |
| 8 | 8 | 0 | 0.125 | 0.125 | 0.125 | 0.125 | 0.125 |
| 8 | 8 | 31 | 0.125 | 0.999 | 0.998 [0.997, 0.999] | 0.999 [0.998, 0.999] | 0.998 [0.998, 1.000] |
| 16 | 1 | 0 | 0.062 | 0.062 | 0.062 [0.062, 0.065] | 0.062 | 0.062 |
| 16 | 1 | 31 | 0.062 | 0.999 [0.999, 1.000] | 0.999 [0.999, 1.000] | 1.000 [0.999, 1.000] | 0.999 |
| 16 | 8 | 0 | 0.062 | 0.062 | 0.062 | 0.062 | 0.062 |
| 16 | 8 | 31 | 0.062 | 0.997 [0.997, 0.999] | 0.997 [0.995, 0.999] | 0.997 [0.997, 0.998] | 0.996 [0.996, 0.997] |

Reading: a value of exactly 1/W means one worker holds every combining
cycle. CES at 4 workers / heavy 1 is the only unsaturated cell (light run p50
0.7 µs = uncontended fast path), so chains are short and rotate. CES and the
FC variants have identical burden patterns at balance 0 (all 1/W) for
different reasons (see Surprises 1–2); at balance 31 they diverge completely.
**fcpq's default shows no combiner-burden pattern different from fc** (both
elected by the `try_lock` race): every cell agrees within 0.003.

### H-B: bursty cells, combiner-worker bystander p99 − non-combiner p99 vs one pass (µs)

| workers | heavy | balance | lock | combiner p99 | non-combiner p99 | difference | pass | verdict |
|---|---|---|---|---|---|---|---|---|
| 4 | 8 | 0 | ces | 283.0 [268.1, 402.1] | 297.9 [283.0, 312.8] | -14.9 [-14.9, 89.4] | 42.8 | within spread |
| 4 | 8 | 0 | fc / fcpq | 268.1 | 0.1 | 268.0 | 138.6 | exceeds pass (convergence, see text) |
| 4 | 8 | 0 | fc-noyield / fcpq-noyield | 2 196 123 (censored) | 0.1 | censored | 36.7 [36.7, 240.6] | starvation |
| 4 | 8 | 31 | ces | 283.0 [253.2, 297.9] | 327.7 [253.2, 1251.0] | -29.8 [-997.9, 29.8] | 42.9 | below pass |
| 4 | 8 | 31 | fc / fcpq | 126.6–134.0 | 134.0 | -7.4–0.0 | 135 | below pass |
| 8 | 1 | 0 | ces | 863.8 [863.8, 893.6] | 863.8 [863.8, 923.4] | 0.0 [-29.8, 0.0] | 39.1 | below pass |
| 8 | 1 | 0 | fc | 186.2 [163.8, 186.2] | 171.3 [156.4, 186.2] | 14.9 [-22.3, 29.8] | 39.1 | below pass |
| 8 | 1 | 0 | fcpq | 238.3 [186.2, 238.3] | 93.1 [0.1, 186.2] | 145.2 [0.0, 238.2] | 38.9 | within spread |
| 8 | 1 | 31 | ces | 893.6 [834.0, 923.4] | 863.8 [863.8, 923.4] | -29.8 [-29.8, 59.6] | 39.2 | within spread |
| 8 | 1 | 31 | fc / fcpq | 54.0 | 55.9 | -1.9 | 39.4 | below pass |
| 8 | 8 | 0 | ces | 44.7 | 44.7 | 0.0 | 136.3 | below pass |
| 8 | 8 | 0 | fc / fcpq | 268.1 | 0.1 | 268.0 | 138.8 | exceeds pass (convergence) |
| 8 | 8 | 0 | fc-noyield / fcpq-noyield | 2 196 123 (censored) | 0.1 | censored | 240.7 | starvation |
| 8 | 8 | 31 | ces | 44.7 | 44.7 | 0.0 | 136.6 | below pass |
| 8 | 8 | 31 | fc / fcpq / noyield | 29.8 | 29.8 | 0.0 | 137–142 | below pass |
| 16 | 1 | 0 | ces | 14.9 [14.9, 15.8] | 14.9 | 0.0 [0.0, 0.9] | 41.6 | below pass |
| 16 | 1 | 0 | fc | 238.3 [186.2, 238.3] | 46.5 [0.2, 171.3] | 191.8 [14.9, 238.1] | 39.7 | within spread |
| 16 | 1 | 0 | fcpq | 238.3 | 0.1 [0.1, 0.7] | 238.2 | 37.7 | exceeds pass (convergence) |
| 16 | 1 | 31 | all five | 14.9–16.8 | 14.9–16.8 | 0.0 | 40.7–41.7 | below pass |
| 16 | 8 | 0 | ces | 2 196 119 (censored) | 14.9 | censored | 140.8 | starvation |
| 16 | 8 | 0 | fc / fcpq | 268.1 | 0.1–0.2 | 268.0 | 140.1 | exceeds pass (convergence) |
| 16 | 8 | 0 | fc-noyield / fcpq-noyield | ≥ 1 952 105 (censored) | 0.1–1.0 | censored | 37.7–242 | starvation |
| 16 | 8 | 31 | ces | – (no bystander on the combiner) | 14.9 | – | 140.7 | – |
| 16 | 8 | 31 | fc / fcpq / noyield | 1.8–2.3 | 1.8–2.3 | 0.0 | 137–144 | below pass |

Full table (every lock row separately): `scripts/summarize.py`, section
"H-B". Reading: under balancing no combining variant ever puts its
combiner-worker bystander p99 above the non-combiner p99 (differences are
0 ± one bucket, and at 4 workers / heavy 1 the ±1 ms swings are within the
repeat spread because bystanders there wait behind ~10 ms of client polls on
every worker). The only "exceeds pass" verdicts arise (a) at balance 0 for
FC-style locks, where the number is the bystander waiting behind all 16
converged clients on its worker (268 µs ≈ 16 × 14.5 µs of parallel work plus
CSs), and (b) CES at 16 workers / heavy 8 / balance 0, where the chain never
ends even with 16 clients and 32× parallel work (15 workers return clients
faster than the chain serves them) and the combiner's bystander is starved
for the entire window.

### H-C: heavy ratio 8, service Jain and cost

| contention | workers | balance | dispatch J | ces J | fc J | fcpq J | fc-noyield J | fcpq-noyield J | fcpq light/heavy ops | fcpq/fc thr | fcpq/ces thr |
|---|---|---|---|---|---|---|---|---|---|---|---|
| sus | 4 | 0 | 0.649 | 0.601 [0.583, 0.602] | 0.649 | 0.650 | 0.016 | 0.016 | 1.00 | 1.00 | 0.69 |
| sus | 4 | 31 | 0.659 | 0.638 [0.583, 0.646] | 0.656 | 0.720 [0.719, 0.720] | 0.656 | 0.719 [0.719, 0.721] | 1.44 [1.44, 1.45] | 1.12 | 1.18 |
| sus | 8 | 0 | 0.650 | 0.574 [0.557, 0.627] | 0.649 | 0.649 | 0.016 | 0.016 | 1.00 | 1.00 | 0.58 |
| sus | 8 | 31 | 0.661 [0.661, 0.664] | 0.651 | 0.658 | 0.713 [0.713, 0.714] | 0.657 | 0.712 [0.711, 0.713] | 1.38 [1.37, 1.39] | 1.10 | 1.14 |
| sus | 16 | 0 | 0.652 | 0.609 [0.607, 0.652] | 0.652 | 0.651 | 0.016 | 0.016 | 1.00 | 1.00 | 0.61 |
| sus | 16 | 31 | 0.666 | 0.655 [0.653, 0.655] | 0.662 | 0.714 [0.712, 0.714] | 0.662 | 0.714 [0.712, 0.715] | 1.35 | 1.08 | 1.13 |
| bur | 4 | 31 | 0.670 | 0.623 [0.619, 0.629] | 0.676 | 0.679 [0.678, 0.680] | 0.791 [0.777, 0.843] | 0.801 [0.798, 0.827] | 1.14 | 1.00 | 0.88 |
| bur | 8 | 31 | 0.673 [0.672, 0.675] | 0.678 [0.678, 0.680] | 0.660 | 0.672 | 0.660 | 0.674 | 1.08 | 1.01 | 0.96 |
| bur | 16 | 31 | 0.679 | 0.656 [0.653, 0.656] | 0.665 [0.664, 0.666] | 0.684 [0.682, 0.685] | 0.665 | 0.687 [0.687, 0.689] | 1.12 | 1.05 | 0.87 |

FIFO baseline check: with equal ops per client, service ∝ cost, and light :
heavy = 1306 : 8324 cycles per op (measured, includes the insert), so the
FIFO Jain is (1+0.157)²/(2(1+0.157²)) = 0.653 — exactly what dispatch, CES and
FC report. FC-PQ moves the light:heavy ops ratio from 1.00 to 1.35–1.45; equal
service would need 6.4. The remaining gap is the policy's own anti-starvation
clamp (8 passes) and newcomer initialisation to the running mean, not the
executor: the fcpq and fcpq-noyield columns agree within 0.002.

fcpq tax vs fc at heavy ratio 1 (throughput medians, Mops/s):

| contention | workers | balance | fc | fcpq | fcpq/fc | fc-noyield | fcpq-noyield | ratio |
|---|---|---|---|---|---|---|---|---|
| sus | 4 | 31 | 1.072 [1.071, 1.072] | 1.055 [1.051, 1.059] | 0.98 | 1.078 [1.075, 1.080] | 1.054 [1.052, 1.057] | 0.98 |
| sus | 8 | 31 | 1.044 [1.037, 1.050] | 1.019 [1.016, 1.029] | 0.98 | 1.042 [1.041, 1.045] | 1.019 [1.016, 1.025] | 0.98 |
| sus | 16 | 31 | 1.001 [0.987, 1.007] | 0.967 [0.965, 0.976] | 0.97 | 0.993 [0.992, 0.993] | 0.973 [0.968, 0.983] | 0.98 |
| bur | 4 | 31 | 0.252 | 0.252 | 1.00 | 0.259 | 0.259 | 1.00 |
| bur | 8 | 31 | 0.461 [0.460, 0.462] | 0.458 [0.457, 0.458] | 0.99 | 0.490 [0.489, 0.491] | 0.489 [0.486, 0.489] | 1.00 |
| bur | 16 | 31 | 0.554 [0.548, 0.557] | 0.567 [0.566, 0.568] | 1.02 | 0.557 [0.554, 0.561] | 0.553 [0.550, 0.553] | 0.99 |

**fcpq's tax vs fc at heavy 1 is 0–3 %**, not above 10 %. (Balance-0 cells
are omitted from this table: they sit in the one-worker collapse regime where
throughput is 0.37–0.39 Mops/s for every FC variant and the ratio is 1.00 by
construction; bursty balance-0 ratios 0.62–0.85 come from cells whose
throughput swings 2× between repeats.)

### Surprises

1. **FC/FC-PQ without a cooperative yield collapse to a single client** when
   the executor does not balance: service Jain 0.016 = 1/64, 63 of 64
   clients with zero ops for 2 s, in every sustained cell (15 of 16 in
   bursty). The winning combiner's `run` returns `Ready` synchronously, the
   waiters it served are woken into *its* worker's local queue, and the
   harness task loop (`run().await; spin`) never returns `Pending`, so the
   same task re-publishes and re-wins an empty election forever. CES cannot
   do this: its unlocker always suspends (`reschedule_self_remote`) when a
   waiter exists. Combiners added `yield_after_combine` (default on) the same
   day; with it, balance-0 FC/FC-PQ instead converge *all* clients onto the
   combiner's worker (burden 1/W, throughput 0.235 vs 0.390 Mops/s with
   balancing) — a fairness fix, not a placement fix.
2. **The executor's remote-drain policy decides whether a CES chain
   persists.** v1 of this matrix (archived in `results/v1-single-item-drain.tar.zst`)
   drained one injector item per 31-poll maintenance tick; with no idle worker
   that caps remote reschedules at (W−1)/(31 polls) ≈ the chain's own rate at
   W ≤ 8, so the wait queue emptied periodically and CES showed burden Jain
   0.96 at 8 workers but 0.06 at 16. Draining len/W+1 per tick (tokio's rule)
   restores 1/W at every W. The lock-level fact is robust — a CES chain ends
   only when the wait queue is empty — but *how often that happens is an
   executor property*.
3. **dispatch under bursty load runs at 0.057–0.071 Mops/s versus CES
   0.315** (4.5–5.5×, all spreads ≤ 2 %) with 16 clients and 14.5 µs of
   parallel work per op: the woken owner sits in the unlocker's local queue
   behind the unlocker's own continuation (or waits for a balancing steal),
   so the handoff latency is about one parallel-work slice per op. CES's
   remote reschedule of the unlocker is what keeps the parallel work parallel.
4. **CES at balance 0 traps whatever sits in the combiner's local queue**:
   5–6 clients and the pinned bystander starve for the whole window at 8
   workers / heavy 8 (service Jain 0.574 [0.557, 0.627] is contaminated;
   throughput spread [0.363, 0.413] is the widest in the matrix). With
   balancing the same cell has 0 starved and Jain 0.651.
5. **Under balancing the combiner worker of CES hosts no bystander at all**
   (`–` in the tables): its pinned bystander is stolen during warm-up and,
   since nothing ever yields onto that worker again, never returns. The F2
   effect therefore appears as *eviction* rather than as delay; bystander p99
   on the other workers rises from 0.8 µs (dispatch) to 3.0 µs (CES) because
   the finished clients' parallel work lands there.
6. FC-PQ is 8–12 % *faster* than FC at heavy ratio 8 under balancing because
   serving light clients more often yields more ops per second
   (light:heavy ops 1.35–1.45); the per-op combining cost is unchanged (tax
   ≤ 3 % at heavy 1).

### Limitations (executor model)

- **Bystanders are always runnable.** They spin then `yield_now`, so no
  worker ever idles and steal-when-idle — the only balancing mechanism in
  tokio-style executors — never fires. Real bystanders are I/O-driven and
  intermittently runnable; idle gaps would let stealing rescue trapped tasks.
  To keep the comparison meaningful the executor adds a rate-limited
  balancing steal (`balance_interval`, default 31 polls, sweep {0, 31}
  here). This knob bounds how long any task can sit in a busy worker's queue
  and therefore caps the measurable F2 delay; at 0 the F2 effect is total
  starvation, which the harness records as one censored sample per starved
  bystander plus `starved_clients`.
- Balance 0 for FC-style locks measures a one-worker system (all clients on
  the combiner's worker); those cells are reported but not interpreted as
  lock properties.
- Latencies are bucket lower bounds (6 % resolution); differences of one
  bucket are not significant. TSC is assumed synchronised across sockets.
- 3 repeats only; spreads are min/max, not confidence intervals. One
  unrelated single-threaded process was present.

### Next question

Phase 3 (H-D): which mitigation restores burden Jain ≥ 0.9 for CES without
giving up its bursty-mode throughput advantage over dispatch — bounding the
inline chain (after K handoffs or T µs, hand off with default placement so
the worker drains its local queue), or rotating the combiner per pass — and
what it costs per op. For H-C: how far the FC-PQ starvation clamp (8 passes)
and newcomer initialisation must be relaxed to reach light:heavy ops ≈ 6.4
(service Jain ≥ 0.95), and whether that survives the executor.
