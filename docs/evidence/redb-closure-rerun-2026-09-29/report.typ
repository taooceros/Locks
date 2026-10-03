// redb closure-API rerun report, written for HTML output:
//   typst compile --features html --format html report.typ report.html
// PDF still works (typst compile report.typ); paged-only styling is gated on target().
// Data: medians.json and summary.png come from make_figures.py (see RESULTS.md).
#set document(
  title: "Delegation locks inside redb: closure-API rerun",
  author: "Locks project",
  date: datetime(year: 2026, month: 9, day: 29),
  description: "redb write-lock experiment rerun on the closure-API build: throughput, service-time fairness and changes from the fixed-body build.",
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
#let clients = (1, 2, 4, 8)
#let get(m, co, du, v, c) = med.at(m + "|" + co + "|" + du + "|" + v + "|" + str(c), default: none)
#let fmt(x, digits) = if x == none { [—] } else { str(calc.round(x, digits: digits)) }
#let kfmt(x) = if x == none { [—] } else { str(calc.round(x / 1000, digits: 1)) + "k" }

// Plain semantic table: header row becomes <thead>, no layout-only strokes.
#let metric-table(m, co, du, cell) = table(
  columns: clients.len() + 1,
  align: (left,) + (right,) * clients.len(),
  table.header([variant], ..clients.map(c => [#c clients])),
  ..variants.map(v => (raw(v),) + clients.map(c => cell(get(m, co, du, v, c)))).flatten(),
)

#title()

Locks project · 2026-09-29 · build `redb-build-06` on merged `main` (#link("https://github.com/taooceros/Locks/pull/54")[PR \#54]); evidence in #link("https://github.com/taooceros/Locks/pull/55")[PR \#55].

= Summary

We reran the redb experiment after PR \#54 replaced the fixed-insert write body with a closure API. All 1,176 timed cells finished (smoke 504, formal 504, perf 168; 0 failures). Every number here is the median of 3 repeats from the formal run, with CPUs 16–23 on NUMA node 0 pinned at 3.0 GHz (power setup S1).

- *FC-PQ is the only variant that is both near-fair and close to the fastest.* Its service-time Jain index is 0.95–1.00 under mixed request sizes. At 8 clients it runs single-record writes at 29.4k tx/s, level with redb's own Mutex/Condvar.
- *U-SCL is perfectly fair (Jain 1.00) but slower,* at 27.9k tx/s with 8 clients.
- *MCS and FC take turns fairly but give unequal service time* (Jain 0.83–0.87). Clients that send 64-record writes get about 70% of the lock time.
- *redb's own lock is neither fair nor stable:* the Jain index falls to about 0.2 at 8 clients, and one client can monopolise the writer.
- *The closure API costs nothing:* the `upstream_gate` path runs at 0.998–1.006× `upstream` wherever there is no contention.

= Setup

The writer lock is the whole redb write transaction, including commit I/O, submitted as a closure (`write_body`: begin → closure → commit or abort). Readers stay outside every write lock.

Variants (the first three are controls without a delegation lock):

/ `upstream`: upstream redb 3.1.0, unmodified; its own Mutex/Condvar writer gate.
/ `upstream_gate`: the closure body in patched redb, admitted through redb's original Mutex/Condvar writer gate.
/ `std_mutex`: the closure body through the bridge, serialised by a `std::sync::Mutex`.
/ `mcs`: libdlock MCS; the body runs on the requester.
/ `uscl`: U-SCL (usage-fair lock); the body runs on the requester.
/ `fc`: flat combining; the body may run on the combiner.
/ `fc_pq`: FC-PQ (usage-ordered combining, with fast path); the body may run on the combiner.

Workloads: `all1` sends one record per transaction. `half1_half64` has half the clients send 1 record and half send 64. `transfer` (smoke only) does 2 reads and 2 updates per transaction. The main durability regime is `None`; `Immediate` (fsync inside the critical section) is a control. The service-time Jain index is computed over each client's time inside the body, measured with rdtscp.

= Results

#figure(
  image(
    "summary.png",
    width: 100%,
    alt: "Six line charts against client count 1 to 8. Top row: tx/s for all1 under None and Immediate, and records/s for half1_half64. Bottom row: service-time Jain index under None and Immediate, and the 64-record clients' service share. FC-PQ stays near the top in throughput and near 1 in Jain; MCS drops most in throughput; the three controls fall toward 1/n in Jain.",
  ),
  caption: [Formal run, median of 3 repeats. Top: throughput. Bottom: fairness under mixed request sizes. The fair long-client share is 0.5.],
)

== Throughput

#figure(
  metric-table("throughput_tx_s", "all1", "none", kfmt),
  caption: [tx/s, `all1`, durability None.],
)

MCS loses the most under contention, 23.9k tx/s at 8 clients. That matches its cache-line transfers in the perf run: about 92–128 HITM loads per transaction, against 8–30 for FC, 9–18 for FC-PQ and about 1 for the controls. Under `Immediate`, fsync dominates and most variants land at 14–14.6k tx/s.

== Fairness

#figure(
  metric-table("service_jain", "half1_half64", "none", x => fmt(x, 3)),
  caption: [Service-time Jain index, `half1_half64`, durability None. With 1 client every index is 1 by definition.],
)

#figure(
  metric-table("long_service_share", "half1_half64", "none", x => fmt(x, 2)),
  caption: [Share of lock time taken by the 64-record clients (fair = 0.5).],
)

The controls' records/s look high in `half1_half64` because whoever holds the lock mostly runs 64-record writes. That is unfairness, not speed: at 8 clients `upstream` completes almost nothing on the short side.

= Changes from the earlier build

The earlier results (redb-internal, change `mnkkkmky`) used the fixed-insert body without LTO.

- *Patched vs `upstream`:* the patched variants used to be 2–6% faster than `upstream` without contention. They are now at parity and execute the same instructions per transaction.
- *Absolute throughput* is 6–9% lower for every variant, `upstream` included, at the same 3.0 GHz. The extra time is outside user mode. The cause has not been attributed, since the two builds were not run side by side.
- *FC-PQ ratios* (`half1_half64`, None, 4 clients, records/s): against U-SCL 0.92 → 1.03, against FC 0.67 → 0.75, against MCS 0.72 → 0.81.
- *FC-PQ's lead under `Immediate` at 8 clients is gone:* it was 1.11× FC and 1.21× MCS, and is now 0.96× and 1.00×. FC-PQ varies by 31% across repeats in those cells.
- *Unchanged:* the fairness ranking, FC-PQ's larger share for long requests, the FC-PQ/FC and FC-PQ/MCS ratios within about 0.05, the fast-path hit rates and the pattern of cache-line transfers.

= Limitations

- Three repeats per point. Some cells ran off the target clock: formal 70 of 475, smoke 112 of 471, perf 18. The runner flags them.
- An intermittent integrity failure remains: the transfer stress with `Immediate` durability on the delegated path needs a repair after reopen about 1 time in 20 (seen on MCS, FC-PQ and U-SCL). It is not root-caused, so `Immediate` transfer results carry that caveat.
- Only power setup S1 could run on this host; S0 and S2 were refused.
- One to eight clients, synthetic key streams; reads are not measured.
- `build.json` records the default workspace's git head rather than the built jj workspace. The binary and source hashes are correct.

Raw data: `.worktree/redb-rerun-06` (formal `analysis/`, smoke `analysis-smoke/`, perf). Details and provenance: `RESULTS.md`.
