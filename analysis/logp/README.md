# Lock analysis: separate performance and fairness models

This manuscript contains **separate performance and fairness analyses**, not another research plan. It analyzes implementation baseline `56a02ab` in the isolated `research/logp-analysis` worktree. Production locks are unchanged.

## Start with the visual story

**[Who gets the next turn?](story-explainer.html)** is the illustration-first
reading companion. It prioritizes explanation over paper length: thirteen
chapters, fifteen numbered visuals, ten new SVG illustrations, and three
optional teaching interactions. The existing LaTeX manuscript is preserved.

The sequence is **lock/CPU scheduling background → unequal service →
existing FIFO/locality/SCL/CFL mechanisms → fairness/performance/work-conservation
tension → independent analyses → delegation → FC-PQ → conditional costs →
limits and evidence**.
Each chapter has a visual explanation, with technical assumptions in
expandable notes. The trilemma is a scoped design tension, not a universal
pick-two theorem: work conservation is distinct from low overhead, and
fairness against inactive clients requires a different contract.

The background separates spinning/blocking from selection policy, and
acquisition order from protected service and lock opportunity. The
existing-solutions chapter explains how SCL accounts, penalizes, and slices,
and how CFL reorders waiters by virtual hold time with NUMA awareness.
CFL's default, strict-guarantee, and grace-period policies make the
tradeoffs concrete. These descriptions concern the primary papers, not
assumed configuration or certified guarantees of the local implementations.

- **Service scheduler:** step through minimum-service or equal-turn selection,
  changing the longer operation's cost.
- **Executor placement:** run the same `A B A B` service order under caller
  execution or a retained combiner.
- **Cost crossover:** adjust exposed costs and actual mean occupancy; explore
  wins, ties, sparse passes, a strong-locality CFL case, and a per-request
  floor that batching cannot overcome.

These are deterministic toy examples, not benchmarks or a simulation of
current Rust execution. All ten manuscript evidence slots remain open.

### Open and share

Open `analysis/logp/story-explainer.html` directly in a browser. No server,
package installation, build step, or internet connection is needed. Keep
`story-explainer.css`, `story-explainer.js`, and `figures/` beside the HTML
when copying it; retain the linked `.tex` sources for the deeper-reading links.
External publication links require a connection.

The narrative and diagrams also work without JavaScript; native disclosure
controls reveal the technical notes. Narrow screens keep diagrams at readable
size inside horizontally scrollable regions. Browser printing substitutes
the static cost diagram and includes technical notes, including in the
tested no-JavaScript print path.

Sources: [`story-explainer.html`](story-explainer.html),
[`story-explainer.css`](story-explainer.css),
[`story-explainer.js`](story-explainer.js), and
[`figures/explainer/`](figures/explainer/). The three earlier manuscript SVGs
are reused from [`figures/`](figures/).

### Explainer verification

Checked in Chrome 147 with local `file://` loading and networking disabled.
The browser smoke run exercised all three controls, keyboard actions, resets,
limits, all six service-cost settings, win/tie/loss cases, zero costs, the
no-win floor, and fractional occupancy. It found no JavaScript errors,
external requests, broken local assets, missing anchors, or duplicate
evidence-slot IDs. Layout checks covered desktop, 390px mobile, and 320px
document-width overflow; all new SVG text fits its viewBox.

The chapter renderings and mobile controls were visually inspected. The
no-JavaScript reading path retains the static diagrams and native disclosures.
Printing also exposes technical notes without JavaScript; extracted print
text retained all ten evidence identifiers. Independent source reviews
covered narrative/analytical fidelity and control behavior. The print fallback
finding was corrected; SVG arrowheads and tight text boxes were also repaired
during visual review.

Generated verification artifacts, not empirical research results:
- [Browser smoke report](../../.worktree/logp/html-review/verification.json).
- [Printable reading companion](../../.worktree/logp/html-review/story-explainer-print.pdf).
- [No-JavaScript print export](../../.worktree/logp/html-review/story-explainer-nojs-print.pdf).

The browser tests check the teaching UI only. They do not discharge the open
analytical-check, implementation-refinement, calibration, or experiment slots.

The trilemma follow-up reran the browser checks, including the new native
disclosure without JavaScript. Desktop/mobile cards and the revised 26-page
print layout were inspected. This was a narrative revision, not a new
independent analytical review or mathematical-check execution.

The background/existing-solutions follow-up reran those checks with thirteen
chapters and new native background/SCL/CFL disclosures. Revised desktop/mobile
sections and the 31-page print layout were visually inspected. No-script print
text retains the technical notes and all ten evidence identifiers. This
verification covers presentation and teaching controls, not new analytical or
empirical evidence.

## Read the section

- [`paper.tex`](paper.tex): standalone two-column driver and references.
- [`section.tex`](section.tex): shared scope/event definitions, the two analysis inputs, and a final discussion of their interface.
- [`performance.tex`](performance.tex): communication and execution model, throughput and return-latency results, performance-only lock comparison, and calibration criteria.
- [`fairness.tex`](fairness.tex): service metric and eligibility contract, fairness proofs and counterexamples, fairness-only lock comparison, and refinement criteria.
- [`implementation-audit.tex`](implementation-audit.tex): source evidence, complete target inventory, legacy/dispatch qualifications, and verification boundary.
- [`check_model.py`](check_model.py): deterministic standard-library Python research artifact.
- Generated PDF: [`../../.worktree/logp/paper.pdf`](../../.worktree/logp/paper.pdf).
- Generated check report: [`../../.worktree/logp/verification.json`](../../.worktree/logp/verification.json).

Generated files live in the ignored `.worktree/` area. The TeX and Python sources are the reproducible deliverable.

## Analysis-led paper-story draft with placeholders

**Fair Service, Local Execution: Usage-Fair Delegation Locks** develops the
user-approved story without replacing the separate analyses. It now follows
**current practice → usage-fair locks → their remaining service/executor
coupling → analysis framework → delegation as a response**. The abstract and
introduction establish the usage-fair-lock problem before introducing
delegation. Broader combining/TCLocks context and SCL's prior fair-delegation
suggestion appear in the design section. FC-PQ instantiates the design;
CFL remains the principal fair caller-executed comparison.

- [`story-draft.tex`](story-draft.tex): problem-first narrative, independent service/execution framework, analysis-motivated FC-PQ design, three SVG concept illustrations, compact CFL comparison, and related work.
- [`story-evaluation.tex`](story-evaluation.tex): four evaluation questions with explicit missing-evidence slots.
- [`story-details.tex`](story-details.tex): optional technical appendix with full cost accounting, fairness and residency proofs, implementation obligations, and evidence boundaries.
- Generated companion PDF: [`../../.worktree/logp/story-draft.pdf`](../../.worktree/logp/story-draft.pdf).

The **seven-page illustrated PDF** has five pages of main narrative/references
and two pages of technical appendix. The main text has two numbered equation displays:
the paired cost model and the conditional CFL crossover. It distinguishes
**performance advantage over CFL** from **incremental fairness cost versus
matched delegation**. It does not claim universal superiority, automatic L1
residency, or unconditional fairness of current FC-PQ.

The **10 unique placeholders** are:

- Motivation and novelty: `MOTIVATION-RESULT`, `NOVELTY-POSITION`.
- Evaluation: `EVAL-SETUP`, `EVAL-SERVICE`, `EVAL-CFL` (the sole result-figure placeholder), `EVAL-COST`, `EVAL-APPLICATION`.
- Technical obligations: `IMPLEMENTATION-REFINEMENT`, `ANALYTICAL-CHECKS`, `CALIBRATION-CONTRACT`.

### Editable concept illustrations

- [`figures/service-allocation.svg`](figures/service-allocation.svg): aligned short/long-operation timelines distinguish equal turns from equal accumulated service. The rows intentionally have different operation mixes; they are not a throughput comparison.
- [`figures/execution-placement.svg`](figures/execution-placement.svg): the same service order under caller execution and a retained combiner, with potential protected-state movement and remaining request/result traffic.
- [`figures/cost-crossover.svg`](figures/cost-crossover.svg): schematic `d + a/b` amortization against `H = h + m`, including the cost floor and no-strict-win regime.

All three are conceptual illustrations, not measurements. Delegation appears
only in the design figure after the problem and framework. SVGs use editable
text and vector shapes, DejaVu Sans with a sans-serif fallback, and explicit
labels in addition to color. No external images or scripts are embedded.

No empirical result is filled in. The existing eight-group checker does not
verify the companion's residency result or CFL-specific comparison; new finite
checks remain placeholders. No simulator, production repair, calibration, or
experiment was performed as part of this illustration revision.

### Reproduce the illustrated draft

[`render_figures.py`](render_figures.py) exports all SVGs to vector PDFs in
the ignored `.worktree/logp/figures/` directory. It needs Python 3 and librsvg's
`rsvg-convert`; no Python packages are required. Install the converter on
`PATH`, or pass `--converter /path/to/rsvg-convert`. The exporter resolves paths
relative to itself, so it can also be invoked outside the worktree.

From the worktree root (standard LaTeX `graphicx`, `balance`, and `placeins`):

```sh
python3 analysis/logp/render_figures.py
latexmk -cd -pdf -interaction=nonstopmode -halt-on-error -outdir=../../.worktree/logp analysis/logp/story-draft.tex
```

The earlier analysis-led version underwent independent claim and narrative
reviews. The problem-first order and SCL attribution in the design section
remain intact. This illustration revision received build and visual checks,
not a new independent review or mathematical-check execution.

Export was exercised with librsvg 2.61.4 from both the worktree and `/tmp`.
All three SVGs and all seven PDF pages were visually inspected; the final
reference page has balanced columns. `pdfimages -list` reports no raster
images, and all ten evidence-slot IDs appear exactly once in the build log.
There are no undefined references or horizontal overfull/underfull boxes.
The known `showhyphens` compatibility warning remains, along with a
non-clipping 1.6873pt vertical-box warning during reference-page balancing.
The evaluation, technical appendix, production code, environment
configuration, original separate-analysis manuscript, and check runner are
unchanged by this illustration revision.

## Two independent analytical tracks

### Performance analysis

**Question:** what work and communication does a given policy generate, and when does that execution improve throughput or return latency?

Inputs are protocol events, operation mix, actual batch occupancy, executor changes, and machine costs. Outputs are:

1. A typed event DAG and resource/dependency lower bound.
2. Trace accounting for request traffic, serialized scheduling, data/metadata migration, and idle time.
3. Intra-epoch ownership invariance and resident-footprint migration amortization.
4. A conditional batch crossover and its intersection with a caller-return-delay certificate.
5. A performance-only comparison of the implemented lock families and a held-out calibration strategy.

No service-fairness theorem is a premise. A service policy is an input, not a guarantee inferred from its name.

### Fairness analysis

**Question:** which service metric is distributed among which eligible clients, and what disparity or competing-service bound follows?

Inputs are the selectable set, admission/initialization rules, selection policy, and billed/useful service increments. Outputs are:

1. An approximate-min service-spread theorem and bounded-accounting-error corollary.
2. A billed-versus-useful-service bias result and limits of final JFI.
3. Service-volume/selection-count waiting and FIFO announcement-visibility bounds.
4. A fairness-only lock-family comparison and source-specific admission, visibility, and accounting counterexamples.

These proofs use no LogP parameters or throughput result. Waiting is in service/selection units, not an automatic wall-clock deadline.

### Final discussion only: relating the outputs

A policy's resulting operation mix and execution pattern can be supplied to the performance model if separately justified. Converting a service-count guarantee into elapsed response time requires additional progress and timing bounds. Neither analysis silently proves the other.

Batching and min-credit principles are not claimed as novel. Shared source evidence and a shared check runner do not merge the two models or their validation criteria.

## Consequential implementation findings

- **FC-SL has a correctness prerequisite:** callers read the shared non-atomic `total_usage`/`total_served` counters before acquiring the combiner mutex while the combiner writes them. This is a source-level data race, not simply timing noise. The appendix gives the conflicting paths; no production fix is hidden in the analysis.
- **FC-PQ's starvation clamp is inert:** it takes the minimum of an already popped minimum and the next queue minimum. Its newcomer baseline and temporary visibility gaps also prevent quoting an unconditional `C_max` fairness bound.
- **Budgets are not interchangeable:** DLock2 FC-PQ caps 64 pops, CC caps 64 services, CC-Ban caps 16 services, and DSM's post-execution test permits 65 services. Legacy CC/CC-Ban have yet different total counts.
- **Legacy `FCBanSlice` does not enforce the advertised slice:** its cutoff is commented out; the active behavior is full scanning plus adaptive waiting.
- **RCL is present in legacy dispatch:** its separate server-aware benchmark reserves a server resource; the exported DLock2 RCL module is not a DLock2 CLI target.

## Reproduce

From the worktree root, using Python 3.9+ and a LaTeX installation with `latexmk`, `pdflatex`, AMS packages, `booktabs`, `longtable`, `lmodern`, `microtype`, `hyperref`, and `balance`:

```sh
python3 analysis/logp/check_model.py --output .worktree/logp/verification.json
latexmk -cd -pdf -interaction=nonstopmode -halt-on-error -outdir=../../.worktree/logp analysis/logp/paper.tex
```

The checker fails with nonzero exit status on a violated condition. Its checks use explicit exceptions rather than Python `assert`, so optimization must not disable them. An optional deterministic/optimized-mode check is:

```sh
python3 -O analysis/logp/check_model.py --output .worktree/logp/verification-optimized.json
cmp .worktree/logp/verification.json .worktree/logp/verification-optimized.json
```

The repository's prescribed `devenv` invocation failed during the preceding analysis on an undefined `dotenv.resolved` option. The arithmetic artifact and typesetting use existing host tools; no environment configuration was changed. There is no new Python dependency to install.

## Executed artifact checks

Both standard and `python3 -O` runs passed all eight checks, and `cmp` confirmed byte-identical JSON reports:

| Track | Check | Cases | Meaning |
|---|---|---:|---|
| Performance | `crossover` | 5,120 | Exact-rational batch inequalities |
| Performance | `batch_frontier` | 5,832 | Brute-force versus algebraic feasible integer batches |
| Fairness | `spread` | 44,400 | Small integer approximate/exact-min invariant transitions |
| Fairness | `accounting_error` | 164,421 | Pairwise credit-error transitions |
| Fairness | `waiting_bound` | 22,587 | Competing-service transitions and terminal target choices |
| Fairness | `charge_bias` | 256 | Equal-billed-service complete cycles |
| Fairness | `announcement_visibility` | 1,485 | FIFO drain/service schedules |
| Cross-checks | `counterexamples` | 7 | Source-inspired and metric counterexample families |

The counts have different meanings; summing them does not produce a count of independent Rust tests. Parameter domains, assumptions, and example witnesses are explicit in the code and JSON report. Finite enumeration supplements the written inductive/algebraic proofs; it is not an unbounded proof or a weak-memory model checker.

The separated 14-page manuscript builds successfully, and all pages were visually inspected. The final log has no undefined references, overfull boxes, or column-balancing warnings; it retains the non-blocking `showhyphens` package-compatibility warning. Independent review found no material cross-dependency between the two analyses. Both Python modes passed all eight check groups with byte-identical reports.

## Evidence boundary

**Established here:** conditional mathematical results, source-grounded cost/event distinctions, explicit proof obligations, finite arithmetic checks, and a reproducible manuscript.

**Not established here:** Rust memory safety/linearizability, executed adversarial Rust schedules, hardware-calibrated model accuracy, comparative throughput numbers, bounded real-world preemption/wakeup time, or an unconditional fairness theorem for current FC-PQ under dynamic arrivals.

For a complete systems-paper evaluation, calibrate primitive costs independently, instrument actual batch/discovery/ownership events, control operation mix and placement, and predict held-out workloads. The current FC-PQ combiner timer is not a full-pass measurement; no analysis here silently treats it as one. A maximum observed sample is not a guaranteed worst-case bound.

This artifact is sufficient to supply a substantive, reviewable theoretical section. It is not a claim that the entire systems paper is submission-ready without empirical evaluation.
