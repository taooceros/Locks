// redb scalability and reader/writer report, written for HTML output:
//   typst compile --features html --format html report.typ report.html
// PDF still works (typst compile report.typ); paged-only styling is gated on target().
// Data: medians.json and the PNGs come from make_figures.py (see RESULTS.md).
#set document(
  title: "Delegation locks inside redb: 64 clients and concurrent readers",
  author: "Locks project",
  date: datetime(year: 2026, month: 9, day: 30),
  description: "redb write-lock experiment extended to 1-64 writer clients across two sockets and to concurrent read transactions: throughput, service-time fairness, collapse, reader throughput and latency.",
)
#set heading(numbering: "1.")

// `target()` exists only with `--features html`; plain PDF builds are always paged.
#let target = if "target" in dictionary(std) { std.target } else { () => "paged" }

// Paged output only: page geometry and typographic tuning that HTML ignores.
#show: body => context {
  if target() == "paged" {
    set page(paper: "us-letter", margin: 2.2cm, numbering: "1")
    set text(size: 10.5pt)
    set par(justify: true)
    show table: set text(size: 9pt)
    body
  } else {
    body
  }
}

// HTML output only: a small stylesheet for readable tables and figures.
#context if target() == "html" {
  html.elem("style", ```
    body { max-width: 60rem; margin: 2rem auto; padding: 0 1rem; line-height: 1.55;
           font-family: system-ui, sans-serif; }
    table { border-collapse: collapse; margin: 0.5rem auto; font-variant-numeric: tabular-nums; }
    th, td { padding: 0.25rem 0.8rem; border-bottom: 1px solid #ddd; }
    thead th { border-bottom: 2px solid #444; }
    td:not(:first-child), th:not(:first-child) { text-align: right; }
    figure { margin: 1.5rem 0; } figcaption { text-align: center; color: #555; font-size: 0.9em; }
    img { max-width: 100%; height: auto; }
    code { background: #f4f4f4; padding: 0 0.2em; border-radius: 3px; }
  ```.text)
}

#let med = json("medians.json")
#let variants = ("upstream", "upstream_gate", "std_mutex", "mcs", "uscl", "fc", "fc_pq")
#let scale-get(m, co, v, w) = med.at("scale|" + co + "|" + m + "|" + v + "|" + str(w), default: none)
#let rw-get(m, w, r, v) = med.at("rw|" + str(w) + "|" + str(r) + "|" + m + "|" + v, default: none)
#let perf-get(m, co, w, r, v) = med.at("perf|" + co + "|" + str(w) + "|" + str(r) + "|" + m + "|" + v, default: none)
#let fixed(x, d) = {
  let s = str(calc.round(x, digits: d))
  if d == 0 { s } else if s.contains(".") { s + "0" * (d - s.split(".").at(1).len()) } else { s + "." + "0" * d }
}
#let fmt(x, digits) = if x == none { [—] } else { fixed(x, digits) }
#let kfmt(x) = if x == none { [—] } else { fixed(x / 1000, 1) + "k" }

// Plain semantic tables: the header row becomes <thead>; one row per variant.
#let vtable(cols, head, cell) = table(
  columns: cols.len() + 1,
  align: (left,) + (right,) * cols.len(),
  table.header([variant], ..head.map(h => [#h])),
  ..variants.map(v => (raw(v),) + cols.map(c => cell(v, c))).flatten(),
)
#let scale-w = (1, 2, 4, 8, 16, 32, 64)
#let scale-head = ([1], [2], [4], [8], [16], [32 (socket full)], [64 (two sockets)])
#let scale-table(m, co, f) = vtable(scale-w, scale-head, (v, w) => f(scale-get(m, co, v, w)))

#title()

Locks project · 2026-09-30 · build `redb-build-02`, branch `experiment/redb-scale-rw`; data, provenance and deviations in `RESULTS.md`.

= Summary

We extended the redb write-path experiment in two directions. First, writer clients go from 8 to 64: up to 32 on one socket (one per physical core), then 64 across both sockets. Second, reader threads run `begin_read` transactions, outside every write lock, while the writers run. All 7 variants, durability `None`, 3 repeats per cell, 540 timed cells in the reported campaign, 0 failed, every cell checked for exact contents and an exact close/reopen. The correctness gate (146 cases, including new cases that run the timed harness with 64 writers and with readers) passed.

- *Throughput is flat from 1 to 64 clients, and only MCS collapses.* One writer runs at a time, so 64 clients cost every other lock 8–15% of its 1-client rate (about 24k tx/s against 28k). MCS falls to 10.6k tx/s at 64 clients, 0.38× its 1-client rate, with repeats from 7.4k to 15.6k.
- *The fairness ordering survives 64 clients and the socket boundary.* FC-PQ and U-SCL keep service-time Jain at 1.000; MCS and FC stay at 0.84–0.90 under mixed request sizes; redb's own lock and `std::Mutex` fall to 0.02–0.08.
- *Readers cannot tell the writer locks apart.* Reader ops/s are within 3–6% across all 7 variants at every reader count, and the p99 is the same histogram bucket. FC and FC-PQ combiners interfere with readers exactly as much as MCS and U-SCL: not measurably.
- *Saturated readers slow every writer lock alike.* With 8 writers, 16 readers leave 18–21% of the writers' tx/s. The cost comes from redb's shared page cache and tracker mutex, not from the write lock.

= Setup

The critical section is one whole write transaction submitted as a closure, as in the closure-API rerun. The variants are unchanged: `upstream`, `upstream_gate` and `std_mutex` are controls; `mcs` and `uscl` run the body on the requester; `fc` and `fc_pq` may run it on a combiner. Workloads: `all1` (every client writes one record per transaction) and `half1_half64` (half the clients write 1 record, half write 64).

*Placement.* The host has two sockets of 32 cores (2 SMT each). Writer number i (from 0) runs on physical core i: CPUs 0–31 are node 0, and the 64-client cells add CPUs 32–63 on node 1. Every cell runs under `numactl --membind=0`, so the clients on node 1 use node-0 memory. Readers sit on other physical cores of node 0.

*Clock.* The power setup is S1 (3.0 GHz fixed, turbo on). The chip holds 3.0 GHz only up to 16 busy cores. With 32 or 64 busy cores every cell runs at 2.80 GHz, and a probe that spins all 64 cores gives 2.80 GHz too. I have no root to lower the target, so those cells are kept and flagged (`clock_off_target`); the drop is the same for every variant.

*Reader.* One read transaction is `begin_read`, the last key of a random writer that has data, 4 point gets of random keys of that writer, and a scan of 16 consecutive keys. Every get must hit with the exact payload and every scan must be consecutive, or the cell fails. Readers have no think time. Ops are gets plus scans (a scan counts as one op; the entries it reads are counted in one extra table below); latency percentiles are histogram bucket upper bounds (at most 12.5% wide).

= Scalability to 64 clients

#figure(
  image(
    "scale.png",
    width: 100%,
    alt: "Six line charts against writer clients 1 to 64 on a log axis, with a dash-dot line between 32 and 64 marking the second socket. Top: tx/s for all1, tx-count Jain, service-time Jain. Bottom: records/s, service-time Jain and the 64-record clients' service share for half1_half64. Throughput is nearly flat for all variants except MCS, which falls to about 10 k tx/s at 64. FC-PQ and U-SCL stay at Jain 1; the three controls fall toward 0.",
  ),
  caption: [Median of 3 repeats. The dash-dot line separates one socket (left) from two (right). Dotted lines are the controls, solid requester-run locks, dashed combiners. Clients above 32 run on node 1 against node-0 memory; cells at 32 and 64 clients ran at 2.8 GHz.],
)

== Throughput

#figure(
  scale-table("throughput_tx_s", "all1", kfmt),
  caption: [tx/s, `all1`, durability None. The 32- and 64-client cells ran at 2.8 GHz, the others at about 3.0.],
)

No lock scales, because the write path is serial; the question is what each lock loses. The controls and U-SCL, FC and FC-PQ lose 8–15% between 1 and 64 clients. MCS loses 16% already at 2 clients (23.3k), 27% at 16, and then 62% at 64 clients. Its 64-client cells range from 7.4k to 15.6k tx/s, so the collapse is large and unstable. In the perf cohort at 64 clients MCS has 336 LLC misses per transaction, against 142–149 for FC and FC-PQ and 2.4 for MCS on one socket. Because the body runs on the requesting thread, I expect the clients on node 1 to run it against node-0 memory [INFERENCE].

#figure(
  scale-table("tx_s_at_ref", "all1", kfmt),
  caption: [tx/s scaled to 3.0 GHz (tx/s × 3.0 / measured clock). It assumes the serial path scales with the clock. U-SCL cells with sleeping clients have no decidable clock (—).],
)

== Fairness

#figure(
  scale-table("service_jain", "all1", x => fmt(x, 3)),
  caption: [Service-time Jain index, `all1`. At 1 client every index is 1 by definition.],
)

#figure(
  scale-table("service_jain", "half1_half64", x => fmt(x, 3)),
  caption: [Service-time Jain index, `half1_half64`.],
)

#figure(
  scale-table("long_service_share", "half1_half64", x => fmt(x, 2)),
  caption: [Share of lock time taken by the 64-record clients (fair = 0.5).],
)

The ordering measured at 8 clients holds to 64, including across the socket boundary. FC-PQ and U-SCL give every client equal service time at 16, 32 and 64 clients (the long clients get 0.50 of the lock time). MCS and FC take turns fairly, so transaction counts are equal, but the long clients get 66–71% of the lock time. The three controls let a few clients monopolise the lock: Jain 0.02–0.08 from 32 clients on, with unchanged tx/s. That makes their `half1_half64` records/s arbitrary (24k to 631k across W) because it depends on whether short or long clients win the lock.

FC-PQ is the only variant with Jain 1.000 and `all1` tx/s within 5% of the fastest at 64 clients (24.3k against 25.4k for `std_mutex`). In `half1_half64` records/s at 64 clients it is 1.08× U-SCL and 1.15× MCS but 0.74× FC, whose lead is the larger lock share given to 64-record clients.

#figure(
  image(
    "clock.png",
    width: 100%,
    alt: "Two line charts of the mean writer-CPU clock against writer clients 1 to 64. All variants sit near 3.0 GHz up to 16 clients and drop to 2.8 GHz at 32 and 64.",
  ),
  caption: [Mean clock of the writer CPUs (sampler). The 3.0 GHz target holds to 16 busy cores; 32 and 64 busy cores run at 2.8 GHz.],
)

= Readers and writers

#figure(
  image(
    "readers.png",
    width: 100%,
    alt: "Eight line charts. Top row, 8 writers against 0 to 16 readers: reader ops/s rising from 0.5M to 1.7M, reader p99 rising from 27 to 120 microseconds, writer tx/s falling from 26k to 5k, writer service Jain. Bottom row, 8 readers against 1 to 8 writers: reader ops/s, reader p99, writer tx/s and mean staleness, all nearly flat. In every panel the lines of the 7 variants lie on top of each other, apart from the Jain panel where the three controls are low.",
  ),
  caption: [Top: 8 writers (`all1`), 0–16 readers. Bottom: 8 readers, 1–8 writers. Median of 3. The 7 variants lie on one curve in every panel except writer Jain.],
)

== Reader throughput and latency against the writer lock

#figure(
  vtable((1, 4, 8, 16), ([R = 1], [R = 4], [R = 8], [R = 16]), (v, r) => kfmt(rw-get("reader_ops_s", 8, r, v))),
  caption: [Reader ops/s (gets + scans; a scan is one op), 8 writers.],
)

#figure(
  vtable((1, 4, 8, 16), ([R = 1], [R = 4], [R = 8], [R = 16]), (v, r) => fmt(rw-get("reader_entry_ops_s", 8, r, v) / 1e6, 2)),
  caption: [Same work counting every entry a scan reads (gets + scanned entries), million per second, 8 writers. Each read transaction is 4 gets and 16 scanned entries.],
)

#figure(
  vtable((1, 4, 8, 16), ([R = 1], [R = 4], [R = 8], [R = 16]), (v, r) => fmt(rw-get("reader_txn_p99_us", 8, r, v), 1)),
  caption: [Read-transaction p99 in µs (bucket upper bound), 8 writers. Buckets are 12.5% wide, so 114.7 and 122.9 are adjacent buckets.],
)

#figure(
  vtable((1, 4, 8, 16), ([R = 1], [R = 4], [R = 8], [R = 16]), (v, r) => fmt(rw-get("reader_stale_mean", 8, r, v), 2)),
  caption: [Mean snapshot staleness in records behind (sampled on every 16th read transaction), 8 writers. The largest gap seen in any cell was 32 at R = 1; all other cells stayed at 10 or less.],
)

The reader columns are flat across variants. At every reader count the highest and lowest variant differ by 3–6%, which is the size of the repeat-to-repeat range (3–5%). The p99 falls in the same or neighbouring histogram buckets. Reader throughput grows sub-linearly with R (0.5M ops/s for one reader, 1.7M for 16): the readers compete with each other for redb's shared structures.

== Do combiners interfere with readers differently?

No measurable difference. FC and FC-PQ run the writer body on a combiner thread that spins; MCS and U-SCL run it on the requester. Reader ops/s of `fc` and `fc_pq` over `mcs` and `uscl` is 0.97–1.02× at every R, inside the noise. Writer tx/s relative to R = 0 is the same for every variant, and the HITM loads per writer transaction at R = 16 are 2.3–3.6k for every variant (perf cohort, readers included). The controls, which block instead of spin, lie on the same curve.

This holds for readers on their own physical cores. I did not try readers on the SMT siblings of a spinning combiner, where spinning might steal issue slots.

== Saturated readers slow the writers, whatever the lock

#figure(
  vtable((0, 1, 4, 8, 16), ([R = 0], [R = 1], [R = 4], [R = 8], [R = 16]), (v, r) => kfmt(rw-get("throughput_tx_s", 8, r, v))),
  caption: [Writer tx/s, 8 writers (`all1`), against the number of saturated readers.],
)

#figure(
  vtable((1, 2, 4, 8), ([W = 1], [W = 2], [W = 4], [W = 8]), (v, w) => kfmt(rw-get("throughput_tx_s", w, 8, v))),
  caption: [Writer tx/s with 8 saturated readers, against the number of writers.],
)

#figure(
  vtable((1, 2, 4, 8), ([W = 1], [W = 2], [W = 4], [W = 8]), (v, w) => fmt(rw-get("service_jain", w, 8, v), 3)),
  caption: [Writer service-time Jain with 8 saturated readers.],
)

Adding readers costs the writers a fixed share of their throughput that does not depend on the lock. At W = 8 they keep 0.74–0.80× of their R = 0 tx/s with one reader, 0.50–0.59× with 4, 0.37–0.42× with 8 and 0.18–0.21× with 16. With 8 readers the writers sit at 8.7–10.3k tx/s for every W from 1 to 8 and every variant; even a single writer drops from 28k to 9k. Differences between the locks that exist without readers (MCS at about 0.85× of the others) mostly vanish under this read load.

One diagnostic `perf record` on HITM loads (MCS, `all1`, W = 2, R = 8; a single cell, not part of the cohort) puts 48% of the cross-core loads of modified lines in redb's `PagedCachedFile::read` and 17% in a contended `std::sync::Mutex`. So the interference is inside redb's shared page cache and its tracker mutex, which the write lock does not protect. That reading rests on one cell [INFERENCE].

The service-fair locks keep their fairness with readers running: writer service Jain stays at 0.997 or above for MCS, U-SCL, FC and FC-PQ at every R and W.

== Cache counters

#figure(
  table(
    columns: 7,
    align: (left, right, right, right, right, right, right),
    table.header([variant], [W=32 HITM/tx], [W=64 HITM/tx], [W=32 LLC/tx], [W=64 LLC/tx], [W=64 IPC], [W=64 tx/s]),
    ..("mcs", "fc", "fc_pq").map(v => (
      raw(v),
      fmt(perf-get("hitm_loads_per_tx", "all1", 32, 0, v), 0),
      fmt(perf-get("hitm_loads_per_tx", "all1", 64, 0, v), 0),
      fmt(perf-get("llc_miss_per_tx", "all1", 32, 0, v), 1),
      fmt(perf-get("llc_miss_per_tx", "all1", 64, 0, v), 1),
      fmt(perf-get("ipc", "all1", 64, 0, v), 2),
      kfmt(perf-get("throughput_tx_s", "all1", 64, 0, v)),
    )).flatten(),
  ),
  caption: [Perf cohort, `all1`, process totals per committed writer transaction (user mode). Medians of 3. The `half1_half64` rows and the W = 8, R = 16 rows for all 7 variants are in `RESULTS.md`.],
)

At 32 clients MCS has 115 HITM loads per transaction, FC 24 and FC-PQ 16. Across the socket boundary the LLC misses per transaction jump by two orders of magnitude for all three (2.4 → 142–336), which is where the remote-socket traffic appears. IPC is 0.05–0.13 for all three because waiters spin.

= Limitations

- Three repeats per cell; ranges are not confidence intervals. MCS at 64 clients varies by 2× between repeats.
- The 32- and 64-client cells ran at 2.8 GHz, not 3.0. Comparisons between variants at the same W are clean; comparisons across the 16 → 32 step mix in a 6.7% clock drop.
- Memory is bound to node 0 in every cell. Interleaved memory would change the 64-client cells.
- Readers are saturated (no think time), which is an extreme load. A paced reader would show a smaller writer slowdown; it was not measured.
- The first reader design picked a random writer for each read transaction. Unfair locks leave most writers without keys, so most of a reader's transactions were empty and reader ops/s depended on the lock. That run (540 cells) is discarded; readers now choose among writers that have data. The reported campaign is the second one.
- Empty read transactions (no data yet at the start of a cell) are 0.025% of all read transactions, at most 0.09% for any reader, so they do not bias the latency percentiles.
- Staleness is a record-count gap (redb has no public read-transaction ID), sampled on every 16th read transaction.
- The perf counters are process totals: in the R = 16 cells the readers' instructions and misses are included.
- The gate's first run had one failure, the known intermittent `transfer-immediate` integrity repair, this time on `std_mutex`. It is not root-caused. That case passed 12 of 12 times on an immediate re-run, and a second full gate run passed 146 of 146.
- `Immediate` durability, the transfer cohort with readers, and real traces were not run.

Raw data: `.worktree/redb-scale-rw-02` in the `redb-scale-rw` workspace. Details and provenance: `RESULTS.md`.
