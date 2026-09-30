// FC-PQ pass-length (H) ablation in redb, written for HTML output:
//   typst compile --features html --format html report.typ report.html
// PDF still works (typst compile report.typ); paged-only styling is gated on target().
// Data: medians.json and summary.png come from make_report_data.py (see RESULTS.md).
#set document(
  title: "Why FC-PQ beats FC in redb: combiner tenure",
  author: "Locks project",
  date: datetime(year: 2026, month: 9, day: 29),
  description: "Ablation of FC-PQ's per-pass cap H (64, active, 8) in the redb write path: throughput, cache-line transfers, bodies per pass, combiner changes, fairness and tail latency.",
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
#let variants = ("fc", "fc_pq", "fc_pq_hn", "fc_pq_h8", "upstream_gate")
#let delegated = ("fc", "fc_pq", "fc_pq_hn", "fc_pq_h8")
#let clients = (1, 2, 4, 8)
#let get(m, co, v, c) = med.at(m + "|" + co + "|" + v + "|" + str(c), default: none)
#let kfmt(x) = if x == none { [—] } else { str(calc.round(x / 1000, digits: 1)) + "k" }
// Fixed number of decimals (Typst's round drops trailing zeros).
#let fmt(x, digits) = if x == none { [—] } else {
  let s = str(calc.round(x, digits: digits))
  if digits > 0 {
    let parts = s.split(".")
    let frac = if parts.len() > 1 { parts.at(1) } else { "" }
    parts.at(0) + "." + frac + "0" * (digits - frac.len())
  } else { s }
}
// Plain semantic tables: header row becomes <thead>, no layout-only strokes.
#let sweep-table(m, co, cell, vs: variants) = table(
  columns: clients.len() + 1,
  align: (left,) + (right,) * clients.len(),
  table.header([variant], ..clients.map(c => [#c clients])),
  ..vs.map(v => (raw(v),) + clients.map(c => cell(get(m, co, v, c)))).flatten(),
)

// The 8-client comparison: one row per variant, one column per measure.
#let eight(co) = table(
  columns: 7,
  align: (left,) + (right,) * 6,
  table.header([variant], [tx/s], [HITM loads/tx], [bodies/pass], [changes /1k bodies], [service Jain], [p99 (ms)]),
  ..delegated.map(v => (
    raw(v),
    kfmt(get("throughput_tx_s", co, v, 8)),
    fmt(get("perf_hitm_loads_per_tx", co, v, 8), 1),
    fmt(get("stats_bodies_per_pass", co, v, 8), 1),
    fmt(get("stats_combiner_changes_per_1k_bodies", co, v, 8), 1),
    fmt(get("service_jain", co, v, 8), 3),
    fmt(get("response_p99_ms", co, v, 8), 2),
  )).flatten(),
)

#title()

Locks project · 2026-09-29 · plan `plan/2026-09-29/fcpq-pass-length-ablation.md`; builds on the closure-API rerun (#link("https://github.com/taooceros/Locks/pull/55")[PR \#55]).

= Summary

In the redb write path FC-PQ beat flat combining (FC) at 8 clients even though it does more work per request. We tested one explanation: *combiner tenure*. FC-PQ pops up to 64 queue entries per pass and puts every served node back, so a node whose client resubmits during the pass is served again and one combiner runs about 44 bodies in a row. FC serves each active node once per pass, about 8 bodies. We capped FC-PQ's pass at the number of active nodes (`fc_pq_hn`) and at 8 (`fc_pq_h8`) and left everything else alone. Two rounds ran 432 timed cells (primary build and counter build) and 48 perf cells with 0 failures, and the gate passed 145 of 145 cases.

- *Held for one-record transactions (`all1`).* With the cap, FC-PQ falls from 26.8k to 24.6k tx/s (`hn`) and 24.5k (`h8`); FC runs 24.8k. HITM loads per transaction go from 10.0 to 21.4 and 20.8 (FC 20.5). Bodies per pass go from 43.5 to 7.8, and combiner changes per 1,000 bodies from 9.4 to 51 and 56 (FC 54). Service-time Jain stays at 0.998 or higher.
- *Only partly held for the mixed cohort (`half1_half64`).* The cap moves the locality numbers exactly the same way (HITM 11.5 to 29.2, equal to FC), but throughput does not fall: 15.6k, 15.8k and 15.7k tx/s against FC at 12.9k. FC-PQ's lead there comes from its serving order, not its tenure: service Jain is 1.000 with the caps, and records per second drop.
- *Long tenure costs tail latency.* The 99th-percentile response at 8 clients is 2.6 ms (`all1`) and 6.3 ms (`half1_half64`) with H = 64, against 0.7 ms and 1.3 ms with the caps, even though the median response at H = 64 is lower.
- *The default did not change:* `fc_pq` keeps H = 64 and an interleaved A/B against the previous build shows no difference.

= Setup

The lock runs whole redb write transactions, as in the closure-API rerun. Workloads: `all1` (every client writes one record per transaction) and `half1_half64` (four clients write 1 record, four write 64). Durability is None. CPUs 16–23 on NUMA node 0, pinned at 3.0 GHz, 2 s per cell, three repeats with random variant order. Every number is a median of three.

Variants:

/ `fc`: flat combining, one sweep over the active nodes per pass.
/ `fc_pq`: FC-PQ with H = 64 pops per pass, unchanged.
/ `fc_pq_hn`: FC-PQ with H = the entries in its queue at pass start (about one per enrolled node).
/ `fc_pq_h8`: FC-PQ with H = 8.
/ `upstream_gate`: control; redb's own Mutex/Condvar writer gate.

The cap is a runtime field read once per pass, so one binary hosts all three FC-PQ variants. Bodies per pass and combiner changes come from a separate build with counters; HITM comes from the perf cohort (8 clients); throughput, fairness and latency come from the timed build without counters.

= Results

#figure(
  image(
    "summary.png",
    width: 100%,
    alt: "Six panels. Top: tx/s for all1 and for half1_half64 against client count, and service-time Jain for half1_half64. Bottom: bodies per combining pass and combiner changes per 1000 bodies for all1, and HITM loads per transaction at 8 clients. FC-PQ with H=64 separates from FC, fc_pq_hn and fc_pq_h8 only at 4 to 8 clients: higher throughput, about 44 bodies per pass instead of 8, fewer combiner changes and half the HITM loads. In half1_half64 the capped variants keep the throughput lead and have Jain 1.0.",
  ),
  caption: [Median of 3 repeats. FC-PQ at H = 64 (thick red) differs from the capped variants and FC in bodies per pass, combiner changes and HITM. Its throughput lead survives the caps only in the mixed cohort.],
)

== One-record transactions

#figure(
  eight("all1"),
  caption: [`all1`, 8 clients, durability None. p99 is the merged response time (bucket upper bound). `upstream_gate` ran 26.7k tx/s with service Jain 0.14 and is omitted.],
)

#figure(
  sweep-table("throughput_tx_s", "all1", kfmt),
  caption: [tx/s, `all1`. At 1 client every variant is the same fast path. The FC-PQ lead appears only when passes get long (4 and 8 clients).],
)

#figure(
  sweep-table("stats_bodies_per_pass", "all1", x => fmt(x, 1), vs: delegated),
  caption: [Bodies per combining pass, `all1` (stats build).],
)

The probability that the combiner changes between two passes is about the same for every variant: 0.41 at H = 64, 0.43 for FC, 0.40 for `hn`, 0.44 for `h8`. The pass length therefore sets how often the combiner, and with it the B-tree working set, moves. In this cohort throughput moves with HITM: dropping the cap returns FC-PQ to FC's throughput and to FC's cache-line transfers.

== Mixed transactions

#figure(
  eight("half1_half64"),
  caption: [`half1_half64`, 8 clients, durability None.],
)

#figure(
  sweep-table("throughput_records_s", "half1_half64", kfmt, vs: delegated),
  caption: [Records per second, `half1_half64`. The capped variants complete fewer records than H = 64 and FC, because they split service time equally (Jain 1.000) and so serve more of the cheap 1-record transactions.],
)

Capping the pass changes every tenure metric but not tx/s. The lead over FC (+21% at H = 64, +23% and +22% with the caps) is from serving order: usage-ordered service gives the 64-record clients less of the time. We did not run an order-only ablation, so this is an inference from Jain and records per second.

== Tail latency

With H = 64 every worker's p99 at 8 clients is 2.6–2.9 ms (`all1`), while its median is 0.12–0.16 ms; FC and the capped variants have 0.33 ms medians and 0.5–0.7 ms p99. In `half1_half64` the H = 64 p99 is 5.2–9.4 ms against 0.8–1.6 ms with `hn`. Long tenure trades a lower typical response for a long tail.

= Limitations

- Three repeats per point; ranges are not confidence intervals. FC-PQ's repeat range in the mixed cohort is 5%, so differences of ±3% between H = 64 and the capped variants are not resolved. The first round (an earlier build of the same sources) agrees on every `all1` number and on every tenure metric; its mixed-cohort lead retained 84–86% instead of 106–109%.
- 6 of 120 timed cells ran 2–3% below the 3.0 GHz target (one of them is the `all1` median of `fc_pq_h8`).
- The counter build's throughput differs from the primary build's by −5% to +3% with inconsistent sign, so its cost is not resolved; throughput comes from the build without counters.
- Bodies per pass is a ratio of totals from the counter build. p50 and p99 are bucket upper bounds with 12.5% resolution. perf counters are process totals and include spinning waiters.
- This isolates tenure inside FC-PQ only. It says nothing about the announcement ring, node layout or priority order.
- Not run: Immediate durability, other mixes, the transfer workload, other clock setups.

Raw data: `.worktree/fcpqh-*-02` (round 2) and `-01` (round 1) in the `fcpq-h-ablation` workspace. Details, provenance and every table: `RESULTS.md`, `tables.md`.
