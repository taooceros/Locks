# redb perf-02: clock-normalised counters and perf overhead

Status: Approved 2026-09-28 (user task assignment, a follow-up of
[redb-internal-rerun](./redb-internal-rerun.md)). Done: implemented, run
and analysed in workspace `redb-internal` (change `mnkkkmky`, PR #50).
Scope: `integration/redb/run.py` and `README.md`. No Rust changed, so build
`.worktree/redb-build-03` and its manifest hashes are reused unchanged.
Extended 2026-09-29 with power setups and the fixed 3.0 GHz (S1) rerun. That
rerun used build-04, whose binaries are identical to build-03's (section
"Fixed clock, S1 at 3.0 GHz").

## Goal

The formal matrix showed two throughput levels in `all1` at 4-8 clients.
HardProblemSolver traced them to the per-core CPU clock (base 2.2 GHz vs turbo).
Without root the clock cannot be pinned. So: record it per cell, normalise or
reject cells, measure what the extra counters cost, and re-read the lock ratios.

## Changes (`integration/redb/run.py`)

1. **Events.** `cycles:u` was already collected; `ref-cycles` was not. The
   runner adds `ref_cycles` = `cpu_clk_unhalted.ref_tsc:u` (encoding 0x0300,
   fixed counter 2). perf 7.2.5 resolves the plain name `ref-cycles` to the
   programmable `cpu_clk_unhalted.ref_tsc_p` (0x013c, `perf stat -vv`), so the
   runner names the fixed event. It records perf's running time and fraction per
   event and cell (`perf_running`) and fails a cell with any event below 100 %,
   keeping the raw result.
2. **TSC rate.** Recorded at preparation from CPUID 0x15 (`cpuid` tool): 25 MHz
   crystal × 176/2 = **2.200 GHz**. Each cell also measures it (`rdtscp` against
   `Instant` over the window): 2.20019 [2.20019, 2.20021] GHz over 168 cells.
3. **Analysis** (`--analyze-perf`, clock table in `summary.md`):
   - `effective_ghz` = cycles / ref_cycles × TSC rate. This is the mean user-mode
     clock of all threads, weighted by unhalted time.
   - `ref_busy_cpus` = ref_cycles / TSC ticks of the window, i.e. CPUs busy in
     user mode.
   - cycles/tx (process total, already reported).
   - `tx_s_at_tsc` / `records_s_at_tsc` = window throughput × 2.2 GHz /
     `effective_ghz`.
4. **Per-client clock.** perf stat cannot give per-thread counts for a
   launched command: `--per-thread` needs `-p`/`-t`, and CPU-wide events need
   `perf_event_paranoid` ≤ 0. So during perf cells only, a runner thread on the
   cell's CPUs samples:
   - each client CPU's `cpuinfo_avg_freq` (the kernel's APERF/MPERF over the
     last 4 ms tick) every 50 ms;
   - `/proc/stat` every 200 ms.

   A clock sample counts if its CPU was ≥ 50 % busy in the enclosing 200 ms
   interval. A client's clock is the mean of its accepted samples.
   **Mixed-clock rule, fixed before the rerun:** clients with ≥ 8 samples
   whose clocks differ by more than 1.25× (base against turbo ≥ 3.0 GHz is
   ≥ 1.36). With fewer than two such clients the cell is undecidable, not mixed.
   `*_unmixed` columns exclude mixed cells.

## Risks and checks before the rerun

- **Counter placement** (perf 7.2.5, CPU 40, other socket). There are 8
  general-purpose counters per logical CPU: 8 `branch-misses:u` run at 100 %,
  and 9 multiplex.
  - `cycles:u` takes a general-purpose counter: 8 GP events + `cycles:u`
    multiplex (88-89 %), 8 + `instructions:u` do not. [INFERENCE] Fixed counter
    1 is held by the NMI watchdog (`nmi_watchdog` = 1).
  - The cohort's set is 6 general-purpose + 2 fixed events, so it cannot
    multiplex. The parser still checks every cell.
- **Sampler accuracy** (other socket, busy loops, sampled every 20 ms, off-CPU).
  - Mean `cpuinfo_avg_freq` vs perf cycles/ref_tsc × 2.2: 3.900 vs 3.896,
    3.72 vs 3.67 and 3.77 vs 3.73 GHz on single CPUs; 2.786 vs 2.791 GHz over
    8 CPUs.
  - Clocks change **within** a process (per-CPU means 2.2-3.8 GHz). Hence the
    client clock is a mean, not a median.
  - Idle CPUs report a stale or requested value (800 MHz-2.5 GHz), hence the
    busy filter.
- **Sampler cost.** Each clock read of 8 CPUs takes ≈ 0.12 ms and each
  `/proc/stat` read ≈ 0.54 ms, so ≈ 0.5 % of one CPU. At 8 clients the sampler
  preempts a client; measured below (E1/E0).
- **Normalisation assumption.** The serial path runs at the process mean clock
  and its time scales with the clock. Kernel, memory and wake-up time need not
  scale. Tested below by the clock elasticity.

## Runs (CPUs 16-23, node 0, membind 0, build-03)

- Before: `uptime` load 3.34 at 20:07:08; CPUs 16-23 ≤ 2 % busy over 5 s.
- Functional check first on CPUs 40-47 / node 1, under the shared lock: 3 perf
  cells plus 40 overhead-arm cells, all parsed, all events 100 %. It is not
  reported as data.

```sh
export PYTHONPATH=$PWD CPUS=16,17,18,19,20,21,22,23 L=$HOME/.cache/locks-experiments/measurement.lock
flock --shared "$L" taskset -c $CPUS python3 -m integration.redb.run --prepare-only \
  --build-dir .worktree/redb-build-03 --output-root .worktree/redb-perf-02 --cpus $CPUS --numa-node 0
flock --exclusive "$L" taskset -c $CPUS python3 -m integration.redb.run --perf --output-root .worktree/redb-perf-02
flock --shared "$L" python3 -m integration.redb.run --analyze-perf --output-root .worktree/redb-perf-02
# overhead: root .worktree/redb-perf-overhead-02 prepared the same way, then (same exclusive lock)
taskset -c $CPUS python3 .worktree/overhead/perf_events_overhead.py run .worktree/redb-perf-overhead-02 6
```

- **perf cohort** 20:07:22-20:13:31: **168 cells, 0 failed**, minimum running
  fraction 100.00 % over 168 × 8 events. Load 13.5 at the end, which includes
  its own 8 spinning clients.
- **Overhead experiment** 20:13:31-20:21:52: 240 cells, 0 failed; its 144 perf
  cells all ran at 100 %. Load 6.2 at the end.
- Raw results, both roots, the three throwaway scripts and their printed
  tables are in `~/Locks-artifacts/redb-perf-02/`.

## Results

### Does the `all1` two-level pattern collapse?

Median [min, max] over 3 repetitions, spread = (max − min) / median. The formal-01
timed run (no perf) is shown for reference; it recorded no clock.

| clients | variant | formal-01 timed tx/s (spread) | perf-02 tx/s (spread) | effective GHz | tx/s @2.2 GHz (spread) | cycles/tx (spread) |
|---|---|---|---|---|---|---|
| 1 | native | 31,826 [23,012, 32,198] (29 %) | 31,716 [29,043, 32,084] (10 %) | 3.55 [3.22, 3.58] | 19,738 [19,677, 19,844] (1 %) | 84,306 [83,897, 84,365] (1 %) |
| 1 | bridge_mutex | 33,016 [32,210, 33,025] (2 %) | 32,448 [28,080, 33,662] (17 %) | 3.50 [3.18, 3.68] | 20,119 [19,445, 20,394] (5 %) | 82,035 [81,086, 86,174] (6 %) |
| 1 | mcs | 32,996 [29,536, 33,596] (12 %) | 29,612 [29,181, 34,088] (17 %) | 3.55 [3.12, 3.70] | 20,245 [18,370, 20,546] (11 %) | 81,536 [80,779, 92,312] (14 %) |
| 1 | uscl | 33,100 [30,866, 33,100] (7 %) | 30,398 [25,134, 33,460] (27 %) | 3.27 [2.77, 3.70] | 19,962 [19,922, 20,452] (3 %) | 81,367 [81,083, 83,638] (3 %) |
| 1 | fc | 33,128 [31,090, 33,344] (7 %) | 29,026 [27,919, 33,228] (18 %) | 3.12 [3.09, 3.64] | 20,073 [19,896, 20,460] (3 %) | 82,095 [81,271, 84,027] (3 %) |
| 1 | fc_pq | 33,136 [30,550, 33,190] (8 %) | 31,951 [20,669, 32,390] (37 %) | 3.49 [2.20, 3.50] | 20,378 [20,143, 20,667] (3 %) | 81,096 [79,830, 82,443] (3 %) |
| 2 | native | 30,248 [28,955, 30,604] (5 %) | 28,776 [28,616, 29,850] (4 %) | 3.47 [3.32, 3.70] | 18,911 [17,021, 19,068] (11 %) | 85,863 [84,737, 97,206] (15 %) |
| 2 | bridge_mutex | 30,434 [28,834, 30,708] (6 %) | 30,872 [25,050, 32,402] (24 %) | 3.55 [2.82, 3.75] | 19,156 [18,996, 19,558] (3 %) | 88,650 [86,607, 88,805] (2 %) |
| 2 | mcs | 26,452 [23,268, 27,306] (15 %) | 23,660 [21,682, 25,293] (15 %) | 3.20 [2.95, 3.50] | 16,185 [15,899, 16,246] (2 %) | 239,373 [238,554, 244,935] (3 %) |
| 2 | uscl | 28,822 [28,568, 31,846] (11 %) | 27,248 [25,837, 29,354] (13 %) | 3.06 [2.93, 3.22] | 19,599 [19,370, 20,088] (4 %) | 103,606 [101,025, 103,768] (3 %) |
| 2 | fc | 31,762 [30,281, 32,038] (6 %) | 28,922 [26,122, 32,094] (21 %) | 3.20 [3.00, 3.64] | 19,416 [19,166, 19,909] (4 %) | 198,275 [193,738, 201,936] (4 %) |
| 2 | fc_pq | 31,360 [29,782, 31,846] (7 %) | 27,032 [26,842, 29,878] (11 %) | 3.05 [3.01, 3.07] | 19,354 [19,349, 21,850] (13 %) | 198,509 [176,459, 199,250] (11 %) |
| 4 | native | 11,834 [7,833, 19,685] (100 %) | 19,284 [18,066, 19,330] (7 %) | 2.19 [2.18, 2.19] | 19,429 [18,134, 19,445] (7 %) | 85,151 [84,467, 90,882] (8 %) |
| 4 | bridge_mutex | 19,743 [9,223, 25,332] (82 %) | 20,020 [18,242, 22,362] (21 %) | 2.10 [1.96, 2.36] | 20,849 [20,504, 20,976] (2 %) | 86,836 [85,268, 90,226] (6 %) |
| 4 | mcs | 16,809 [13,459, 25,037] (69 %) | 17,910 [14,518, 22,809] (46 %) | 2.38 [2.24, 3.31] | 15,148 [14,237, 16,590] (16 %) | 542,299 [496,717, 577,416] (15 %) |
| 4 | uscl | 9,473 [9,108, 11,416] (24 %) | 9,432 [9,172, 9,967] (8 %) | 0.99 [0.95, 1.08] | 21,031 [20,362, 21,252] (4 %) | 99,057 [97,545, 101,226] (4 %) |
| 4 | fc | 19,958 [18,203, 27,254] (45 %) | 19,843 [19,710, 23,722] (20 %) | 2.20 [2.20, 2.71] | 19,708 [19,242, 19,873] (3 %) | 418,988 [415,250, 428,815] (3 %) |
| 4 | fc_pq | 22,792 [19,884, 25,726] (26 %) | 19,740 [19,370, 25,973] (33 %) | 2.20 [2.20, 2.97] | 19,369 [19,268, 19,761] (3 %) | 426,471 [417,425, 428,474] (3 %) |
| 8 | native | 10,811 [8,619, 26,298] (164 %) | 18,839 [8,348, 19,518] (59 %) | 2.10 [0.90, 2.19] | 19,738 [19,614, 20,394] (4 %) | 83,300 [83,041, 83,950] (1 %) |
| 8 | bridge_mutex | 8,558 [8,495, 27,772] (225 %) | 17,114 [8,088, 17,598] (56 %) | 1.81 [0.87, 1.84] | 20,543 [20,432, 21,341] (4 %) | 86,744 [85,932, 90,691] (5 %) |
| 8 | mcs | 16,890 [16,560, 22,872] (37 %) | 16,950 [14,022, 17,738] (22 %) | 2.20 [2.20, 2.35] | 16,614 [14,021, 16,949] (18 %) | 1,024,017 [1,004,786, 1,213,434] (20 %) |
| 8 | uscl | 8,109 [7,939, 8,582] (8 %) | 7,976 [7,967, 8,392] (5 %) | 0.88 [0.85, 0.89] | 20,534 [19,996, 20,720] (4 %) | 100,569 [99,896, 102,265] (2 %) |
| 8 | fc | 24,920 [19,733, 28,202] (34 %) | 20,648 [19,686, 27,590] (38 %) | 2.30 [2.20, 3.07] | 19,725 [19,685, 19,798] (1 %) | 862,855 [860,954, 864,633] (0 %) |
| 8 | fc_pq | 27,557 [20,478, 29,226] (32 %) | 20,366 [20,054, 23,548] (17 %) | 2.21 [2.20, 2.43] | 20,368 [19,963, 21,282] (6 %) | 836,370 [801,474, 852,822] (6 %) |

**Yes, except for MCS.**

- **FC, FC-PQ.** At 4-8 clients the tx/s spread is 17-38 % (26-45 % in
  formal-01). At 2.2 GHz it is 1-6 %, and cycles/tx varies by 0-6 %. The fast
  repetitions are the turbo ones: FC-PQ c4 25,973 tx/s at 2.97 GHz becomes
  19,761.
- **Mutex/Condvar controls.** Their swings (56-59 % here, 82-225 % in
  formal-01) are clock levels too; at 2.2 GHz they shrink to 2-7 %.
  - When the lock rotates between sleeping clients, every core is mostly idle
    and runs at 0.87-0.90 GHz. Native c8, repetition 0: 8,348 tx/s at
    0.90 GHz with `service_jain` 0.745, against 19,518 tx/s at 2.19 GHz with
    0.133 in repetition 1.
  - When one client monopolises the lock, its core stays at ≈ 2.2 GHz.
  - The formal-01 reading called these collapses "a different effect". They
    are the same effect, set by who holds the lock. The fairness pattern
    (`service_jain` near 1/c) is still the Mutex's.
- **MCS keeps 16-18 %** at 2.2 GHz, and its cycles/tx varies by 15-20 % at a
  constant 2.20-2.35 GHz (c8). That variation does not come from the clock.
  [INFERENCE: hand-off/migration pattern; not examined.]
- **At 2.2 GHz, `all1` costs the same at every client count.** Every lock
  except MCS gives 18.9-21.0k tx/s from 1 to 8 clients: the serialised body
  costs ≈ 110k cycles per transaction (≈ 50 µs at 2.2 GHz). MCS gives
  15.1-16.6k (0.78-0.84× FC-PQ), matching its ≈ 90-125 HITM loads/tx. The
  cache counters replicate formal-01: HITM loads/tx MCS 89-125, FC 9-30,
  FC-PQ 10-18.

### Lock ratios before and after normalisation

Ratio of medians, raw → at 2.2 GHz, all cells / excluding cells flagged mixed.
Undecidable cells (`mixed_clock` = None) stay in the second figure, so it is not
a "known single-clock" subset. The generated columns labelled "unmixed" mean the
same thing.

| cohort | metric | clients | FC-PQ/FC raw | at 2.2 GHz | FC-PQ/MCS raw | at 2.2 GHz | FC-PQ/U-SCL raw | at 2.2 GHz |
|---|---|---|---|---|---|---|---|---|
| all1 | tx/s | 2 | 0.93 | 1.00 / 1.00 | 1.14 | 1.20 / 1.20 | 0.99 | 0.99 / 0.98 |
| all1 | tx/s | 4 | 0.99 | 0.98 / 0.99 | 1.10 | 1.28 / 1.33 | 2.09 | 0.92 / 0.92 |
| all1 | tx/s | 8 | 0.99 | 1.03 / 1.02 | 1.20 | 1.23 / 1.21 | 2.55 | 0.99 / 0.98 |
| half1_half64 | tx/s | 2 | 0.99 | 1.10 / 1.14 | 1.13 | 1.22 / 1.25 | 0.90 | 0.94 / 0.94 |
| half1_half64 | tx/s | 4 | 1.15 | 1.23 / 1.27 | 1.24 | 1.54 / 1.54 | 2.17 | 0.93 / 1.07 |
| half1_half64 | tx/s | 8 | 1.24 | 1.16 / 1.13 | 1.34 | 1.42 / 1.31 | 2.54 | 0.99 / 0.96 |
| half1_half64 | records/s | 2 | 0.76 | 0.89 / 0.89 | 0.87 | 0.98 / 0.98 | 1.07 | 1.21 / 1.20 |
| half1_half64 | records/s | 4 | 0.63 | 0.68 / 0.70 | 0.69 | 0.86 / 0.86 | 2.44 | 1.05 / 1.05 |
| half1_half64 | records/s | 8 | 0.91 | 0.81 / 0.79 | 0.98 | 0.97 / 0.91 | 3.24 | 1.13 / 1.11 |

- **FC-PQ vs FC:** essentially unchanged.
  - `all1`: 0.98-1.03.
  - `half1_half64`: 1.10-1.23 in tx/s and 0.68-0.89 in records/s. FC-PQ serves
    the 1-record half more, as before.
- **FC-PQ vs MCS:** FC-PQ's lead grows a little.
  - `all1`: 1.10-1.20 → 1.20-1.28.
  - `half1_half64` tx/s: 1.13-1.34 → 1.22-1.54; records/s 0.86-0.98.
- **FC-PQ vs U-SCL:** the raw 2.1-2.6× (`all1`) and 2.4-3.2× records/s
  (`half1_half64`) leads at 4-8 clients **stand**. At 2.2 GHz they become
  0.92-0.99 in tx/s and 1.05-1.13 in records/s, but that figure is a
  decomposition, not a correction (see below).
  - U-SCL's effective clock is 0.84-1.08 GHz at 4-8 clients (sampler
    0.87-1.03 at 4; undecidable at 8), against 3.06-3.32 at 1-2 clients.
  - Its `all1` cycles/tx at 4-8 clients (99-101k) are 20-25 % above the
    lone-client 81k.
  - [INFERENCE] Mechanism: U-SCL waiters sleep, so each core is busy ≈ 1/c of
    the time. schedutil's request (≈ 1.25 × utilisation × max) then sits near
    the 0.8 GHz minimum, and a 2.2 ms slice is too short to ramp up.
  - The low clock is **caused by U-SCL's own design** (waiters sleep, so its
    cores are under-utilised), not an external confound. Normalising it away
    would remove a cost U-SCL imposes on itself. The raw ratio is therefore
    the comparison; the 2.2 GHz ratio only says *where* the gap comes from:
    little per-cycle work difference, most of it the down-clocking that
    sleeping induces. This also resolves the formal-01 puzzle that U-SCL's
    user cycles/tx were far below its body time: the body runs below 1 GHz.
  - Contrast FC/FC-PQ: their two levels vary run to run for the *same* lock
    (host clock state between sessions), which is exogenous, so normalising
    there is correct.
  - The raw gap depends on the DVFS policy; with the `performance` governor
    it would shrink toward the per-cycle ratio [INFERENCE, needs root to test].
  - Caveat: U-SCL never ran above 1.08 GHz at 4-8 clients, so its normalised
    value extrapolates 2.2-2.6×. Its sleep and wake-up latency need not scale
    with the clock, so the value is an upper bound [INFERENCE].
- The mixed-clock exclusion changes these ratios by ≤ 0.06, with two
  exceptions: FC-PQ/MCS `half1_half64` c8 tx/s (1.42 → 1.31) and FC-PQ/U-SCL
  c4 (0.93 → 1.07). Each exception leaves only one unmixed FC-PQ or U-SCL cell.

### Is the normalisation valid?

Clock elasticity of tx/s: the log-log slope within a cell group, over perf-02
plus overhead-arm P8 cells (same configuration, 9 cells per group; only groups
with a ≥ 1.2× clock range).

| cell | clock range (GHz) | slope |
|---|---|---|
| all1 c1 fc_pq | 2.19-3.88 | 0.99 |
| all1 c4 fc | 2.20-3.86 | 0.91 |
| all1 c4 fc_pq | 2.20-3.88 | 0.92 |
| all1 c8 fc_pq | 2.20-3.68 | 0.89 |
| all1 c8 mcs | 2.20-3.43 | 1.10 |
| all1 c8 native | 0.89-2.42 | 0.96 |
| half1_half64 c8 fc_pq | 2.20-3.84 | 0.95 |

Median slope 0.95, i.e. throughput is nearly proportional to the clock. With a
slope of 0.92, normalisation over-corrects a 3.7 GHz cell by ≈ 4 %. The same
configuration in two sessions, 6-14 min apart:

| cell | P8 GHz | perf-02 GHz | raw tx/s ratio P8/perf-02 | ratio at 2.2 GHz |
|---|---|---|---|---|
| all1 c1 fc_pq | 3.80 [2.19, 3.88] | 3.49 [2.20, 3.50] | 1.08 | 0.98 |
| all1 c4 fc | 3.68 [3.64, 3.86] | 2.20 [2.20, 2.71] | 1.61 | 0.96 |
| all1 c4 fc_pq | 3.78 [3.69, 3.88] | 2.20 [2.20, 2.97] | 1.64 | 0.98 |
| all1 c8 fc_pq | 3.45 [3.35, 3.68] | 2.21 [2.20, 2.43] | 1.50 | 0.96 |
| all1 c8 mcs | 3.06 [2.20, 3.43] | 2.20 [2.20, 2.35] | 1.27 | 0.92 |
| all1 c8 uscl | 0.86 [0.85, 0.92] | 0.88 [0.85, 0.89] | 0.97 | 0.97 |
| all1 c8 native | 1.30 [0.89, 2.42] | 2.10 [0.90, 2.19] | 0.62 | 1.01 |
| half1_half64 c8 fc_pq | 3.40 [2.76, 3.84] | 2.33 [2.20, 2.60] | 1.45 | 0.99 |

Minutes apart, the host ran the same cells at 2.2 GHz and then at 3.4-3.8 GHz
(raw 0.62-1.64×). At 2.2 GHz the two sessions agree to 0.92-1.01.

- **Sampler vs perf.** The sampler's client mean over perf's `effective_ghz`
  is 1.001 [0.786, 1.401] (n = 161). The tails are blocking cells, where
  perf weights by busy time.
- **Mixed-clock rule (a heuristic, not a rejection oracle).** 37 of 168 cells
  are flagged mixed: client clocks up to 1.88× apart, typically one core at
  2.2 GHz beside turbo cores. 77 are undecidable: 42 one-client cells and 35
  blocking cells with fewer than two busy clients.
  - Excluding flagged cells barely moves the medians (last two columns of the
    full table below).
  - The sampler covers the whole child process, not only perf's client phase.
    The busy filter drops idle setup, but busy setup or verification on a
    client CPU could qualify. So no conclusion here rests on this flag alone.
- **What `effective_ghz` is not.** It is a process-wide mean over all
  inherited threads. For FC/FC-PQ it is not the combiner's clock when waiters
  run at other clocks, which is exactly the flagged-mixed case.
  - The collapse claim rests on three things, not on the normalisation alone:
    process cycles/tx being invariant (0-6 % for FC/FC-PQ), the 0.95 median
    elasticity, and the cross-session agreement (0.92-1.01).
  - It holds for FC and FC-PQ. For MCS it does not: cycles/tx still spans
    1.00-1.21M at c8. U-SCL and the Mutex controls are set by low clocks on
    sleeping cores, so their normalised values are extrapolations.
- **User-mode share.** A lone client is in user mode for 0.74-0.87 of the
  window (`ref_busy_cpus`). The rest is redb's kernel time (file writes even
  under `None`), which the user-mode counters do not see.

### Overhead: do the counters change throughput?

Interleaved on the same 8 cells (None), 6 repetitions, rotating arm order:

| arm | perf | events | clock sampler |
|---|---|---|---|
| E0 | no | — | no (the timed-cohort configuration) |
| E1 | no | — | yes |
| P6 | yes | instructions + 5 cache events (no cycles, no ref_cycles) | yes |
| P7 | yes | P6 + `cycles:u` (the formal-01 set) | yes |
| P8 | yes | P7 + `ref_tsc:u` (the perf-02 set) | yes |

tx/s median [min, max]; formal-01 and perf-02 are other sessions.

| cell | E0 | E1 | P6 | P7 | P8 | formal-01 timed | formal-01 perf | perf-02 |
|---|---|---|---|---|---|---|---|---|
| all1 c1 fc_pq | 27625 [20011, 35687] | 34519 [20062, 35672] | 34698 [24317, 35414] | 34718 [19728, 35551] | 34577 [19788, 35442] | 33136 [30550, 33190] | 36814 [21605, 36870] | 31951 [20669, 32390] |
| all1 c4 fc | 32605 [30914, 32978] | 32143 [29460, 32984] | 32084 [30989, 32718] | 31939 [29851, 32868] | 31918 [30891, 32654] | 19958 [18203, 27254] | 21767 [21676, 27411] | 19843 [19710, 23722] |
| all1 c4 fc_pq | 32706 [30669, 33298] | 32804 [18212, 33894] | 32570 [20041, 32914] | 32118 [28604, 33252] | 32282 [31342, 33002] | 22792 [19884, 25726] | 21996 [18071, 32735] | 19740 [19370, 25973] |
| all1 c8 fc_pq | 31724 [29969, 33126] | 31479 [19511, 33367] | 31040 [19350, 32101] | 30605 [24470, 32048] | 30583 [29716, 32811] | 27557 [20478, 29226] | 21087 [18767, 26338] | 20366 [20054, 23548] |
| all1 c8 mcs | 19550 [13946, 24906] | 23086 [16115, 25250] | 24183 [21404, 24808] | 23014 [13593, 25434] | 21504 [13619, 24026] | 16890 [16560, 22872] | 17118 [12996, 19344] | 16950 [14022, 17738] |
| all1 c8 uscl | 7902 [7666, 8294] | 7758 [7662, 8250] | 7610 [7535, 7996] | 7791 [7632, 7921] | 7744 [7633, 8400] | 8109 [7939, 8582] | 8452 [7334, 8486] | 7976 [7967, 8392] |
| all1 c8 native | 13774 [8544, 30498] | 14432 [8179, 29706] | 21197 [8183, 29550] | 19470 [8010, 29514] | 11633 [8240, 21581] | 10811 [8619, 26298] | 8649 [7252, 30912] | 18839 [8348, 19518] |
| half1_half64 c8 fc_pq | 18184 [17614, 20205] | 19302 [17999, 19454] | 18606 [11555, 19162] | 18120 [12154, 19902] | 18423 [15380, 20225] | 13702 [12164, 16601] | 13892 [11492, 18223] | 12748 [11586, 14124] |

Paired per-repetition ratios, median [min, max]: 48 pairs raw, 38-40 pairs at
the sampler clock.
"At the sampler clock" divides each side by its sampler client clock (39-40
pairs; not U-SCL, whose clients rarely pass the busy filter, and only some
native pairs).

| comparison | raw | at the sampler clock | per-cell medians (at the sampler clock where available) |
|---|---|---|---|
| P8/P6: cycles + ref_tsc added | 0.997 [0.338, 1.696] | 1.002 [0.696, 1.031] | 0.990-1.011, native 0.949 (n = 3) |
| P7/P6: cycles added | 0.995 [0.562, 1.442] | 0.999 [0.881, 1.054] | 0.983-1.010 |
| P8/P7: ref_tsc added | 1.001 [0.464, 1.516] | — | raw 0.830-1.018 |
| P8/E1: whole perf stat | 0.984 [0.336, 1.756] | 0.992 [0.856, 1.185] | 0.986-0.998, native 1.141 (n = 2) |
| E1/E0: clock sampler | 1.001 [0.589, 1.695] | — (E0 has no clock) | raw 0.945-1.090 |
| P8/E0: perf cohort vs timed | 0.988 [0.555, 1.760] | — | raw 0.841-0.992 |

The noise is the clock. E0's repeat spread is 6-14 % for the FC, FC-PQ and
U-SCL cells and 56-159 % for 1-client FC-PQ, 8-client MCS and native.

- **The two added counters cost nothing measurable.** P8/P6 is 1.002 at the
  sampler clock, and every per-cell median lies within 0.990-1.011 except
  native (0.949 over 3 pairs, raw spread 159 %). A cost above ≈ 1-2 % for the
  spinning locks would show here; U-SCL raw 1.009 and native could hide more.
- **perf stat as a whole** (8 events against none, sampler in both) costs
  ≈ 1 %: median 0.984 raw and 0.992 at the sampler clock. That is within
  the per-cell noise.
- **The sampler** shows no effect (E1/E0 1.001). Its raw per-cell medians
  (0.945-1.090) are not tighter than the clock noise.
- **Against formal-01's timed cohort** raw tx/s differs by up to 1.63× (E0 vs
  formal-01 timed, all1 c4 fc). The host clock differs, not perf. formal-01
  recorded no clock, so a cross-session comparison cannot separate perf's
  cost. Within one session (formal-01 timed vs formal-01 perf, or E0 vs P8
  here) the ranges overlap in all 8 cells.

## Consequences for the rerun report

- The frequency caveat in `redb-internal-rerun.md` is now quantified. For
  FC/FC-PQ the two levels are the clock. At 2.2 GHz, FC-PQ ≈ FC ≈ native ≈
  U-SCL in `all1` throughput, and MCS is 16-22 % lower.
- FC-PQ's raw throughput lead over U-SCL at 4-8 clients is real and caused by
  U-SCL's sleeping waiters: under-utilised cores are down-clocked (0.84-1.08
  GHz). Per cycle the two do similar work; the lead is DVFS-mediated and
  policy-dependent, not per-transaction work.
- The formal matrix was not re-run under S0. The fixed-clock rerun below
  (section "Fixed clock, S1 at 3.0 GHz") records a clock in every timed cell and
  replaces the frequency-confounded formal-01 numbers for the five lock variants.
- Per-thread counters (combiner vs waiters) are still open. They would need
  `perf_event_open` inside the harness, which is a Rust change.

## Fixed clock, S1 at 3.0 GHz (approved 2026-09-29)

### Plan

- **Question.** Under S0, FC-PQ/U-SCL was 2.1-2.6× in `all1` tx/s and 2.4-3.2×
  in `half1_half64` records/s at 4-8 clients. U-SCL's cores then ran at
  0.84-1.08 GHz, and its per-cycle comparison was 0.92-1.13. At a fixed clock
  the two hypotheses predict:
  - **Main's:** the down-clocking of sleeping cores caused the deficit, so the
    ratio falls toward 0.92-1.13.
  - **The user's:** cores switch frequency fast, so a fixed clock removes little
    and the ratio stays ≈ 2-3×.
- **Setups** (operator-applied, no root in the harness; `run.py --power-setup`,
  see README "Power setups"):
  - **S0** is stock.
  - **S1(F):** `performance` governor with min = max = F on all 128 policies,
    turbo on, C6 enabled.
  - **S2(F):** S1 with C6 disabled on CPUs 16-23.
  - The runner refuses a mismatching host at prepare, at every matrix start and
    after every cell.
  - Under S1/S2 a cell whose clock is outside F ± 2 % is flagged, not dropped.
    The clock is perf `effective_ghz` in perf cells and the sampler's client
    mean in timed cells.
  - Normalised throughput uses F as the reference.
- **F.** The sustain probe (`--sustain-probe`, 8 spinners, ~10 s) chooses the
  highest F the shared host holds. Main's probe on CPUs 16-23 held 3.000 GHz on
  every CPU (load 1.2). My 3 s functional probe on 40-47 also gave 3.000. So
  F = 3.0; higher F was not tried.
- **Runs** (CPUs 16-23, node 0, exclusive lock):
  1. S1 perf cohort (168 cells, 7 variants).
  2. S1 timed formal matrix, native/mcs/uscl/fc/fc_pq (`--variants`; 360 cells).
  3. Later: the S2 perf cohort.
- **Comparisons.**
  - S0 → S1 is the DVFS cost of sleeping.
  - S1 → S2 is the C6 wake cost.
  - The target is U-SCL's ≈ 120-200 µs per body under S0 (30-60 µs uncontended).

### Build and checks

- The parent change's libdlock sources (`fc_sl`, unit test) changed after
  build-03, so `load_build` refused it. The rebuild
  `.worktree/redb-build-04` has **byte-identical binaries** (all three SHA-256
  equal to build-03, same patched tree `94132b5f…`). The 64/64 gate and
  165 upstream tests therefore carry over.
- **Preflight** on the pinned host (00:10):
  - `--check-power --power-setup S1 --fixed-ghz 3.0` passes.
  - S0 refuses (128 policy mismatches), S1 at 3.2 refuses (128), and S2
    refuses (8 CPUs with C6 enabled).
  - Before the host was pinned, S0 matched the stock state (all 128 policies
    `schedutil` 800000-3900000).
- **Functional check** on CPUs 40-47 / node 1, shared lock, not data. Four cells
  (perf and timed) all parsed with no power problems. A timed U-SCL cell
  recorded a sampler clock. The 3 s probe gave 3.000 GHz on all 8 CPUs.
- **Regression check:** re-analysing the perf-02 root with the new analyzer gives
  unchanged numbers (e.g. FC-PQ all1 c8 tx/s at 2.2 GHz 20,368).

### Runs

- Before: `uptime` load 1.22 at 00:12:08; CPUs 16-23 0.0 % busy over 5 s.
- `.worktree/redb-perf-03-fixed3g`: 00:12-00:18, **168 cells, 0 failed**, every
  event at 100 % running, 0 power-state changes. Load 3.5 at the end.
- `.worktree/redb-formal-03-fixed3g`: 00:18-00:31, **360 cells, 0 failed**,
  0 power-state changes. Load 3.0 at the end.
- Raw data and scripts: `~/Locks-artifacts/redb-fixed3g/`.

### Clock flags

| cohort | cells | off target (outside 3.0 ± 2 %) | undecidable | flagged cells' clock |
|---|---|---|---|---|
| perf-03 (perf `effective_ghz`) | 168 | **18 / 168 = 10.7 %**, all U-SCL | 0 | 2.85-2.93 GHz |
| formal-03 (sampler) | 360 | **45 / 331 = 13.6 %**: native 22, U-SCL 13, FC-PQ 4, MCS 3, FC 3 | 29 (mostly U-SCL c8) | 2.91-2.94 GHz |

Per-variant clock in perf-03, median [min, max] GHz:

| native | refactored | bridge_mutex | mcs | uscl | fc | fc_pq |
|---|---|---|---|---|---|---|
| 3.001 [3.000, 3.001] | 3.000 [3.000, 3.001] | 3.000 [2.999, 3.001] | 3.001 [2.993, 3.001] | 2.910 [2.846, 3.001] | 2.979 [2.957, 3.001] | 2.989 [2.974, 3.001] |

- With min = max = 3.0 GHz the clock no longer varies between runs. The flagged
  cells are 3-5 % low, not the S0 swings of 0.9-3.9 GHz.
- U-SCL at 2-8 clients sits at 2.85-2.93.
- [INFERENCE] The low cells are cores that wake from C-states often: U-SCL waiters
  and Mutex waiters sleep, and a core runs below F briefly after a C6 exit. S2
  (C6 off) tests this.
- Flagged cells are kept. None of the conclusions below changes if they are
  excluded, because every flagged cell is within 5 % of F.

### Which hypothesis does the data support?

**Main's: FC-PQ/U-SCL falls to the per-cycle value.** FC-PQ over U-SCL, None:

| cohort, metric | clients | S0 formal-01 raw | S0 perf-02 raw | S0 perf-02 at 2.2 GHz | S1 perf-03 raw | S1 formal-03 raw |
|---|---|---|---|---|---|---|
| all1 tx/s | 4 | 2.41 | 2.09 | 0.92 | 1.01 | **1.00** |
| all1 tx/s | 8 | 3.40 | 2.55 | 0.99 | 1.05 | **1.05** |
| half1_half64 records/s | 4 | 2.57 | 2.44 | 1.05 | 1.07 | **0.92** |
| half1_half64 records/s | 8 | 3.47 | 3.24 | 1.13 | 1.25 | **1.27** |
| half1_half64 tx/s | 4 | 2.40 | 2.17 | 0.93 | 0.97 | 0.95 |
| half1_half64 tx/s | 8 | 2.80 | 2.54 | 0.99 | 0.94 | 0.92 |

- At 3.0 GHz the ratio is 0.92-1.27 at 4-8 clients. This is inside or next to
  S0's clock-normalised 0.92-1.13, and far from 2-3×.
  - The 8-client `half1_half64` records/s (1.25-1.27) sits slightly above the
    per-cycle 1.13.
- The fast-switching alternative is ruled out for this host: a fixed clock
  removes the whole deficit.
- U-SCL itself, S0 → S1 (timed, median tx/s):
  - `all1` c4: 9,473 → 31,158 (3.3×); c8: 8,109 → 30,732 (3.8×).
  - `half1_half64` c4: 5,361 → 19,588 (3.7×); c8: 4,902 → 19,176 (3.9×).
  - Its uncontended cells barely change (`all1` c1 33,100 → 33,428).

### U-SCL per body

Per-body time = service utilisation / tx/s. Clock = perf `effective_ghz`.

| cohort | clients | S0 clock (GHz) | S1 clock (GHz) | S0 µs/body (formal-01) | S1 µs/body (formal-03) | S0 cycles/tx | S1 cycles/tx |
|---|---|---|---|---|---|---|---|
| all1 | 1 | 3.27 [2.77, 3.70] | 3.001 | 30 [30, 32] | 29 [29, 29] | 81,367 | 81,816 |
| all1 | 2 | 3.06 [2.93, 3.22] | 2.909 | 34 [31, 34] | 31 [31, 31] | 103,606 | 98,275 |
| all1 | 4 | 0.99 [0.95, 1.08] | 2.866 | **103 [86, 108]** | **31 [31, 35]** | 99,057 | 99,159 |
| all1 | 8 | 0.88 [0.85, 0.89] | 2.884 | **120 [114, 123]** | **32 [32, 36]** | 100,569 | 100,226 |
| half1_half64 | 1 | 3.32 [2.73, 3.88] | 3.001 | 57 [57, 59] | 62 [62, 65] | 180,136 | 179,181 |
| half1_half64 | 2 | 3.16 [2.87, 3.82] | 2.930 | 48 [47, 48] | 49 [49, 54] | 183,094 | 160,667 |
| half1_half64 | 4 | 0.94 [0.94, 0.95] | 2.903 | **183 [179, 192]** | **50 [50, 55]** | 160,931 | 163,734 |
| half1_half64 | 8 | 0.87 [0.84, 0.94] | 2.913 | **200 [174, 202]** | **51 [51, 57]** | 180,867 | 165,884 |

- **The ≈ 200 µs body is explained.** Cycles per transaction do not change
  (≈ 100k `all1`, ≈ 165k `half1_half64`), while the clock goes from < 1 GHz to
  2.9 GHz.
  - U-SCL's contended body now costs the same as its 2-client body
    (31-32 µs `all1`, 49-51 µs `half1_half64`).
  - The remaining 3-5 % below F and any C6 exit latency are what S2 would add
    or remove.

### FC-PQ/FC and FC-PQ/MCS against S0's clock-normalised ratios

| cohort, metric | clients | FC-PQ/FC: S0 at 2.2 → S1 raw (perf-03 / formal-03) | FC-PQ/MCS: S0 at 2.2 → S1 raw (perf-03 / formal-03) |
|---|---|---|---|
| all1 tx/s | 2 | 1.00 → 1.00 / 0.99 | 1.20 → 1.18 / 1.17 |
| all1 tx/s | 4 | 0.98 → 1.01 / 1.00 | 1.28 → 1.18 / 1.18 |
| all1 tx/s | 8 | 1.03 → 1.08 / 1.09 | 1.23 → 1.23 / 1.24 |
| half1_half64 tx/s | 2 | 1.10 → 1.12 / 1.11 | 1.22 → 1.21 / 1.22 |
| half1_half64 tx/s | 4 | 1.23 → 1.26 / 1.24 | 1.54 → 1.36 / 1.34 |
| half1_half64 tx/s | 8 | 1.16 → 1.23 / 1.21 | 1.42 → 1.31 / 1.30 |
| half1_half64 records/s | 2 | 0.89 → 0.87 / 0.86 | 0.98 → 0.94 / 0.94 |
| half1_half64 records/s | 4 | 0.68 → 0.76 / 0.67 | 0.86 → 0.83 / 0.72 |
| half1_half64 records/s | 8 | 0.81 → 0.89 / 0.90 | 0.97 → 0.95 / 0.96 |

- **FC-PQ/FC matches** S0's clock-normalised value within 0.08. This confirms
  that for FC/FC-PQ the S0 normalisation was a correction.
- **FC-PQ/MCS matches within 0.10 except `half1_half64` c4-c8 tx/s** (1.42-1.54
  → 1.30-1.36). [INFERENCE] MCS's non-clock spread (16-18 % at 2.2 GHz under
  S0) made its normalised median noisy.
- **Absolute throughput at 3.0 GHz.** `all1` at 4-8 clients: FC-PQ 31.2k/32.3k,
  FC 31.2k/29.7k, U-SCL 31.2k/30.7k, native 31.2k/31.1k, MCS 26.4k/26.2k tx/s.
  - Repeat ranges are 6-11 %, against 8-164 % for the same five variants in
    formal-01.
  - The two-level pattern is gone without any normalisation.

### service_jain

Unchanged in order and nearly in value (formal-01 → formal-03, None):

| lock | `half1_half64` service_jain at 2/4/8 clients | 64-record half's service share |
|---|---|---|
| FC-PQ | 0.965/1.000/0.963 → 0.941/0.995/0.946 | 0.60/0.51/0.59 → 0.63/0.50/0.61 |
| FC | 0.86/0.84/0.85 → 0.83/0.84/0.85 | — |
| MCS | 0.87/0.88/0.87 → 0.85/0.86/0.86 | — |
| U-SCL | 1.000 at every count, as before | — |

- FC-PQ loses 0.02 at 2 and 8 clients. [INFERENCE] The 1-record half is served
  a little less once the combiner runs as fast as the waiters.
- `half1_half8` and `all1`: every lock except native stays at ≥ 0.989.
- Native stays unfair (0.22-0.58), with a different winner per run.

### Immediate control

FC-PQ ratios at 3.0 GHz are 0.96-1.21 (formal-03, `all1` and `half1_half64` at
4-8 clients), against 0.76-1.36 under S0. fsync dominates, as before.

### Consequences

- The S0 FC-PQ lead over U-SCL at 4-8 clients was caused by governor
  down-clocking of U-SCL's sleeping cores. At a fixed clock, FC-PQ ≈ U-SCL in
  tx/s (0.92-1.05) and 0.92-1.27× in `half1_half64` records/s.
  - On a `schedutil` host the raw S0 ratio is still what U-SCL delivers.
- For the thesis comparison at a fixed clock:
  - FC-PQ vs U-SCL: similar throughput, both fair (FC-PQ service_jain
    0.94-1.00, U-SCL 1.000).
  - FC-PQ vs FC: 1.0-1.1× `all1` tx/s and 1.1-1.25× `half1_half64` tx/s at
    0.67-0.90× records/s, with fairness 0.94-1.00 against 0.83-0.85.
  - FC-PQ vs MCS: 1.17-1.36× tx/s.
- **Open:** the S2 perf cohort (C6 off on 16-23) to price the C6 wake cost and
  the 3-5 % low clock of sleeping-waiter cells. The Immediate control and
  `half1_half8` are in formal-03 but not discussed beyond the above.

## Full clock table (perf-02, generated `analysis-perf/summary.md`)

Median [min, max] over 3 repetitions. The columns are:

- **effective GHz:** perf cycles / ref_tsc × 2.2.
- **client GHz and client spread:** from the sampler.
- **mixed:** mixed cells / cells.
- **user CPUs (ref):** `ref_busy_cpus`.
- **tx/s @TSC:** tx/s at 2.2 GHz. The *unmixed* columns exclude mixed cells.

| cohort | clients | variant | tx/s | effective GHz | client GHz | client spread | mixed | user CPUs (ref) | cycles/tx | tx/s @TSC | tx/s @TSC (unmixed) | records/s @TSC (unmixed) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| all1 | 1 | native | 31716 [29043, 32084] | 3.55 [3.22, 3.58] | 3.60 [3.26, 3.63] | — | 0/3 | 0.75 [0.75, 0.76] | 84306 [83897, 84365] | 19738 [19677, 19844] | 19738 [19677, 19844] | 19738 [19677, 19844] |
| all1 | 1 | refactored | 32068 [27184, 32730] | 3.50 [3.24, 3.66] | 3.51 [3.26, 3.67] | — | 0/3 | 0.75 [0.74, 0.77] | 82281 [82020, 91643] | 19665 [18459, 20166] | 19665 [18459, 20166] | 19665 [18459, 20166] |
| all1 | 1 | bridge_mutex | 32448 [28080, 33662] | 3.50 [3.18, 3.68] | 3.49 [3.21, 3.61] | — | 0/3 | 0.75 [0.75, 0.76] | 82035 [81086, 86174] | 20119 [19445, 20394] | 20119 [19445, 20394] | 20119 [19445, 20394] |
| all1 | 1 | mcs | 29612 [29181, 34088] | 3.55 [3.12, 3.70] | 3.57 [3.12, 3.72] | — | 0/3 | 0.75 [0.75, 0.77] | 81536 [80779, 92312] | 20245 [18370, 20546] | 20245 [18370, 20546] | 20245 [18370, 20546] |
| all1 | 1 | uscl | 30398 [25134, 33460] | 3.27 [2.77, 3.70] | 3.25 [2.78, 3.72] | — | 0/3 | 0.75 [0.74, 0.76] | 81367 [81083, 83638] | 19962 [19922, 20452] | 19962 [19922, 20452] | 19962 [19922, 20452] |
| all1 | 1 | fc | 29026 [27919, 33228] | 3.12 [3.09, 3.64] | 3.19 [3.07, 3.62] | — | 0/3 | 0.76 [0.75, 0.76] | 82095 [81271, 84027] | 20073 [19896, 20460] | 20073 [19896, 20460] | 20073 [19896, 20460] |
| all1 | 1 | fc_pq | 31951 [20669, 32390] | 3.49 [2.20, 3.50] | 3.48 [2.20, 3.50] | — | 0/3 | 0.75 [0.75, 0.75] | 81096 [79830, 82443] | 20378 [20143, 20667] | 20378 [20143, 20667] | 20378 [20143, 20667] |
| all1 | 2 | native | 28776 [28616, 29850] | 3.47 [3.32, 3.70] | 3.49 [3.33, 3.70] | — | 0/3 | 0.74 [0.73, 0.75] | 85863 [84737, 97206] | 18911 [17021, 19068] | 18911 [17021, 19068] | 18911 [17021, 19068] |
| all1 | 2 | refactored | 29038 [27963, 29378] | 3.48 [3.26, 3.70] | 3.48 [3.22, 3.70] | 1.08 [1.08, 1.08] | 0/3 | 0.74 [0.73, 0.75] | 93482 [81717, 93486] | 17656 [17446, 19608] | 17656 [17446, 19608] | 17656 [17446, 19608] |
| all1 | 2 | bridge_mutex | 30872 [25050, 32402] | 3.55 [2.82, 3.75] | 3.57 [3.00, 3.73] | 1.02 [1.01, 1.39] | 1/3 | 0.77 [0.77, 0.77] | 88650 [86607, 88805] | 19156 [18996, 19558] | 19076 [18996, 19156] | 19076 [18996, 19156] |
| all1 | 2 | mcs | 23660 [21682, 25293] | 3.20 [2.95, 3.50] | 3.23 [2.93, 3.48] | 1.00 [1.00, 1.67] | 1/3 | 1.76 [1.76, 1.77] | 239373 [238554, 244935] | 16185 [15899, 16246] | 16072 [15899, 16246] | 16072 [15899, 16246] |
| all1 | 2 | uscl | 27248 [25837, 29354] | 3.06 [2.93, 3.22] | 3.09 [2.94, 3.22] | 1.03 [1.00, 1.67] | 1/3 | 0.92 [0.91, 0.92] | 103606 [101025, 103768] | 19599 [19370, 20088] | 19843 [19599, 20088] | 19843 [19599, 20088] |
| all1 | 2 | fc | 28922 [26122, 32094] | 3.20 [3.00, 3.64] | 3.20 [2.97, 3.66] | 1.00 [1.00, 1.01] | 0/3 | 1.75 [1.75, 1.76] | 198275 [193738, 201936] | 19416 [19166, 19909] | 19416 [19166, 19909] | 19416 [19166, 19909] |
| all1 | 2 | fc_pq | 27032 [26842, 29878] | 3.05 [3.01, 3.07] | 3.07 [3.07, 3.09] | 1.00 [1.00, 1.53] | 1/3 | 1.75 [1.75, 1.75] | 198509 [176459, 199250] | 19354 [19349, 21850] | 19352 [19349, 19354] | 19352 [19349, 19354] |
| all1 | 4 | native | 19284 [18066, 19330] | 2.19 [2.18, 2.19] | 2.19 [2.19, 2.20] | — | 0/3 | 0.75 [0.75, 0.75] | 85151 [84467, 90882] | 19429 [18134, 19445] | 19429 [18134, 19445] | 19429 [18134, 19445] |
| all1 | 4 | refactored | 21307 [20058, 27540] | 2.31 [2.19, 3.41] | 2.32 [2.19, 3.45] | 1.01 [1.01, 1.01] | 0/3 | 0.74 [0.74, 0.76] | 81216 [80701, 93979] | 20135 [17756, 20265] | 20135 [17756, 20265] | 20135 [17756, 20265] |
| all1 | 4 | bridge_mutex | 20020 [18242, 22362] | 2.10 [1.96, 2.36] | 2.20 [1.99, 2.65] | 1.27 [1.15, 1.39] | 1/3 | 0.81 [0.81, 0.85] | 86836 [85268, 90226] | 20849 [20504, 20976] | 20740 [20504, 20976] | 20740 [20504, 20976] |
| all1 | 4 | mcs | 17910 [14518, 22809] | 2.38 [2.24, 3.31] | 2.38 [2.25, 3.32] | 1.12 [1.09, 1.32] | 1/3 | 3.74 [3.73, 3.75] | 542299 [496717, 577416] | 15148 [14237, 16590] | 14692 [14237, 15148] | 14692 [14237, 15148] |
| all1 | 4 | uscl | 9432 [9172, 9967] | 0.99 [0.95, 1.08] | 0.91 [0.87, 1.03] | 1.49 [1.23, 1.69] | 2/3 | 0.94 [0.93, 0.94] | 99057 [97545, 101226] | 21031 [20362, 21252] | 21252 [21252, 21252] | 21252 [21252, 21252] |
| all1 | 4 | fc | 19843 [19710, 23722] | 2.20 [2.20, 2.71] | 2.20 [2.20, 2.70] | 1.01 [1.00, 1.32] | 1/3 | 3.75 [3.75, 3.75] | 418988 [415250, 428815] | 19708 [19242, 19873] | 19791 [19708, 19873] | 19791 [19708, 19873] |
| all1 | 4 | fc_pq | 19740 [19370, 25973] | 2.20 [2.20, 2.97] | 2.20 [2.20, 2.98] | 1.01 [1.00, 1.67] | 1/3 | 3.75 [3.75, 3.75] | 426471 [417425, 428474] | 19369 [19268, 19761] | 19565 [19369, 19761] | 19565 [19369, 19761] |
| all1 | 8 | native | 18839 [8348, 19518] | 2.10 [0.90, 2.19] | 2.16 [1.26, 2.19] | 1.03 [1.03, 1.03] | 0/3 | 0.74 [0.74, 0.78] | 83300 [83041, 83950] | 19738 [19614, 20394] | 19738 [19614, 20394] | 19738 [19614, 20394] |
| all1 | 8 | refactored | 25994 [18802, 28065] | 2.86 [2.03, 3.24] | 2.55 [1.99, 2.97] | 1.17 [1.09, 1.88] | 1/3 | 0.74 [0.74, 0.74] | 81360 [79727, 85913] | 20027 [19032, 20332] | 20180 [20027, 20332] | 20180 [20027, 20332] |
| all1 | 8 | bridge_mutex | 17114 [8088, 17598] | 1.81 [0.87, 1.84] | 1.98 [1.97, 1.99] | 1.14 [1.03, 1.26] | 1/3 | 0.84 [0.80, 0.84] | 86744 [85932, 90691] | 20543 [20432, 21341] | 20488 [20432, 20543] | 20488 [20432, 20543] |
| all1 | 8 | mcs | 16950 [14022, 17738] | 2.20 [2.20, 2.35] | 2.20 [2.20, 2.35] | 1.00 [1.00, 1.20] | 0/3 | 7.73 [7.73, 7.74] | 1024017 [1004786, 1213434] | 16614 [14021, 16949] | 16614 [14021, 16949] | 16614 [14021, 16949] |
| all1 | 8 | uscl | 7976 [7967, 8392] | 0.88 [0.85, 0.89] | — | — | 0/3 | 0.93 [0.92, 0.93] | 100569 [99896, 102265] | 20534 [19996, 20720] | 20534 [19996, 20720] | 20534 [19996, 20720] |
| all1 | 8 | fc | 20648 [19686, 27590] | 2.30 [2.20, 3.07] | 2.30 [2.20, 3.07] | 1.38 [1.00, 1.62] | 2/3 | 7.74 [7.74, 7.75] | 862855 [860954, 864633] | 19725 [19685, 19798] | 19685 [19685, 19685] | 19685 [19685, 19685] |
| all1 | 8 | fc_pq | 20366 [20054, 23548] | 2.21 [2.20, 2.43] | 2.21 [2.20, 2.44] | 1.03 [1.00, 1.67] | 1/3 | 7.74 [7.74, 7.75] | 836370 [801474, 852822] | 20368 [19963, 21282] | 20165 [19963, 20368] | 20165 [19963, 20368] |
| half1_half64 | 1 | native | 14768 [14308, 16360] | 3.35 [3.27, 3.87] | 3.36 [3.25, 3.86] | — | 0/3 | 0.86 [0.86, 0.86] | 201898 [190692, 203217] | 9397 [9311, 9922] | 9397 [9311, 9922] | 305392 [302607, 322458] |
| half1_half64 | 1 | refactored | 14576 [10162, 16462] | 3.22 [2.20, 3.47] | 3.21 [2.17, 3.44] | — | 0/3 | 0.86 [0.85, 0.86] | 186645 [179885, 190754] | 10161 [9975, 10430] | 10161 [9975, 10430] | 330206 [324167, 338960] |
| half1_half64 | 1 | bridge_mutex | 16202 [10461, 16610] | 3.38 [2.20, 3.63] | 3.37 [2.19, 3.61] | — | 0/3 | 0.86 [0.85, 0.86] | 181635 [178745, 186551] | 10460 [10076, 10555] | 10460 [10076, 10555] | 339957 [327449, 343023] |
| half1_half64 | 1 | mcs | 15712 [10673, 17092] | 3.34 [2.19, 3.85] | 3.34 [2.19, 3.83] | — | 0/3 | 0.85 [0.85, 0.85] | 181087 [175194, 192454] | 10340 [9765, 10710] | 10340 [9765, 10710] | 336027 [317348, 348083] |
| half1_half64 | 1 | uscl | 15796 [12879, 17496] | 3.32 [2.73, 3.88] | 3.29 [2.79, 3.86] | — | 0/3 | 0.85 [0.85, 0.85] | 180136 [179080, 189037] | 10387 [9922, 10469] | 10387 [9922, 10469] | 337568 [322444, 340240] |
| half1_half64 | 1 | fc | 15262 [10596, 17088] | 3.36 [2.20, 3.87] | 3.34 [2.19, 3.82] | — | 0/3 | 0.85 [0.85, 0.86] | 188719 [177116, 193240] | 9985 [9715, 10620] | 9985 [9715, 10620] | 324517 [315734, 345161] |
| half1_half64 | 1 | fc_pq | 14770 [10442, 16020] | 3.13 [2.25, 3.38] | 3.09 [2.24, 3.39] | — | 0/3 | 0.86 [0.85, 0.87] | 181575 [180334, 186257] | 10378 [10229, 10418] | 10378 [10229, 10418] | 337277 [332435, 338590] |
| half1_half64 | 2 | native | 27504 [9192, 29704] | 3.13 [3.12, 3.85] | 3.13 [3.10, 3.85] | — | 0/3 | 0.75 [0.74, 0.88] | 96480 [84699, 300034] | 16987 [6469, 19385] | 16987 [6469, 19385] | 19385 [16987, 414035] |
| half1_half64 | 2 | refactored | 10020 [9714, 11989] | 3.13 [3.13, 3.83] | 3.14 [3.14, 3.81] | — | 0/3 | 0.87 [0.87, 0.88] | 278369 [272123, 283227] | 6891 [6835, 7040] | 6891 [6835, 7040] | 441008 [437462, 450559] |
| half1_half64 | 2 | bridge_mutex | 12281 [10586, 18703] | 3.17 [2.59, 3.82] | 3.34 [2.60, 3.80] | 1.38 [1.38, 1.38] | 1/3 | 0.88 [0.85, 0.92] | 274125 [117824, 276834] | 7345 [7066, 15858] | 7206 [7066, 7345] | 452766 [435429, 470102] |
| half1_half64 | 2 | mcs | 14002 [13042, 15590] | 3.36 [2.84, 3.83] | 3.39 [2.84, 3.83] | 1.01 [1.01, 1.59] | 1/3 | 1.85 [1.84, 1.85] | 444061 [401981, 453866] | 9156 [8954, 10088] | 9055 [8954, 9156] | 294286 [291018, 297553] |
| half1_half64 | 2 | uscl | 17682 [14860, 20691] | 3.16 [2.87, 3.82] | 3.19 [2.82, 3.85] | 1.01 [1.01, 1.57] | 1/3 | 0.99 [0.99, 1.01] | 183094 [180572, 191022] | 11931 [11382, 12308] | 12120 [11931, 12308] | 239957 [239855, 240059] |
| half1_half64 | 2 | fc | 16054 [12410, 17139] | 3.49 [2.59, 3.83] | 3.50 [2.60, 3.85] | 1.01 [1.00, 1.36] | 1/3 | 1.85 [1.85, 1.85] | 402042 [385597, 412105] | 10108 [9857, 10560] | 9983 [9857, 10108] | 323977 [320348, 327605] |
| half1_half64 | 2 | fc_pq | 15832 [13874, 19102] | 3.01 [2.89, 3.77] | 3.02 [2.87, 3.78] | 1.18 [1.02, 1.58] | 1/3 | 1.84 [1.83, 1.84] | 360964 [351174, 383174] | 11162 [10571, 11555] | 11359 [11162, 11555] | 287118 [284059, 290177] |
| half1_half64 | 4 | native | 19552 [10968, 29048] | 2.36 [2.17, 3.29] | 2.68 [2.19, 3.35] | 1.22 [1.00, 1.44] | 1/3 | 0.76 [0.75, 0.86] | 86141 [82798, 186056] | 19413 [10212, 19812] | 19613 [19413, 19812] | 19613 [19413, 19812] |
| half1_half64 | 4 | refactored | 8573 [7316, 19210] | 2.20 [2.19, 2.22] | 2.20 [2.20, 2.29] | 1.54 [1.54, 1.54] | 1/3 | 0.87 [0.71, 0.88] | 226790 [80959, 264521] | 8487 [7326, 19317] | 13321 [7326, 19317] | 244079 [19317, 468841] |
| half1_half64 | 4 | bridge_mutex | 16176 [9330, 21147] | 2.55 [2.10, 2.92] | 2.61 [2.21, 3.02] | 1.19 [1.01, 1.37] | 1/3 | 0.88 [0.84, 0.91] | 108711 [105809, 285981] | 16935 [7019, 18275] | 11977 [7019, 16935] | 305441 [161676, 449206] |
| half1_half64 | 4 | mcs | 10270 [8230, 11519] | 2.63 [2.19, 3.09] | 2.63 [2.18, 3.08] | 1.39 [1.02, 1.55] | 2/3 | 3.83 [3.83, 3.85] | 1020404 [986314, 1026591] | 8255 [8215, 8580] | 8255 [8255, 8255] | 268266 [268266, 268266] |
| half1_half64 | 4 | uscl | 5877 [5106, 5900] | 0.94 [0.94, 0.95] | 0.89 [0.87, 0.99] | 1.33 [1.15, 1.54] | 2/3 | 0.99 [0.98, 1.00] | 160931 [158408, 181551] | 13726 [11875, 13783] | 11875 [11875, 11875] | 219706 [219706, 219706] |
| half1_half64 | 4 | fc | 11113 [9667, 14628] | 2.36 [2.20, 2.89] | 2.36 [2.19, 2.91] | 1.11 [1.01, 1.58] | 1/3 | 3.85 [3.84, 3.85] | 817844 [759266, 872697] | 10360 [9680, 11149] | 10020 [9680, 10360] | 329931 [320632, 339230] |
| half1_half64 | 4 | fc_pq | 12736 [12060, 12894] | 2.20 [2.20, 2.20] | 2.20 [2.20, 2.20] | 1.00 [1.00, 1.00] | 0/3 | 3.81 [3.81, 3.82] | 657331 [651735, 695779] | 12745 [12059, 12894] | 12745 [12059, 12894] | 230784 [226233, 250723] |
| half1_half64 | 8 | native | 8379 [5340, 26512] | 2.71 [1.51, 3.00] | 2.44 [1.91, 3.02] | 1.60 [1.60, 1.60] | 1/3 | 0.89 [0.75, 0.89] | 251108 [84838, 286239] | 7769 [6813, 19414] | 13592 [7769, 19414] | 220190 [19414, 420967] |
| half1_half64 | 8 | refactored | 7190 [6900, 9314] | 2.15 [2.09, 2.87] | 2.20 [2.18, 2.39] | 1.23 [1.00, 1.47] | 1/3 | 0.88 [0.87, 0.88] | 267738 [264795, 269855] | 7262 [7143, 7350] | 7306 [7262, 7350] | 464356 [458320, 470391] |
| half1_half64 | 8 | bridge_mutex | 10082 [3928, 10328] | 1.85 [1.02, 2.99] | 1.89 [1.20, 2.64] | 1.35 [1.28, 1.48] | 3/3 | 0.91 [0.87, 0.93] | 235466 [156246, 275424] | 8513 [7426, 12304] | — | — |
| half1_half64 | 8 | mcs | 9516 [8146, 11700] | 2.20 [2.20, 3.05] | 2.20 [2.20, 3.06] | 1.00 [1.00, 1.50] | 1/3 | 7.83 [7.81, 7.83] | 2040876 [1810526, 2109070] | 8438 [8145, 9515] | 8830 [8145, 9515] | 287004 [264756, 309252] |
| half1_half64 | 8 | uscl | 5026 [4556, 5126] | 0.87 [0.84, 0.94] | — | — | 0/3 | 0.98 [0.98, 0.98] | 180867 [172126, 182745] | 12015 [11889, 12648] | 12015 [11889, 12648] | 235609 [218677, 251849] |
| half1_half64 | 8 | fc | 10270 [10263, 11170] | 2.20 [2.20, 2.39] | 2.20 [2.20, 2.39] | 1.00 [1.00, 1.27] | 1/3 | 7.84 [7.84, 7.84] | 1679419 [1678812, 1679880] | 10269 [10265, 10276] | 10267 [10265, 10269] | 330930 [330264, 331596] |
| half1_half64 | 8 | fc_pq | 12748 [11586, 14124] | 2.33 [2.20, 2.60] | 2.33 [2.20, 2.60] | 1.32 [1.01, 1.48] | 2/3 | 7.82 [7.80, 7.83] | 1441164 [1431376, 1480437] | 11949 [11590, 12026] | 11590 [11590, 11590] | 261834 [261834, 261834] |
