# Publication analysis: communication, scheduling, and visibility in locks

Status: completed theoretical-section deliverable, including the user-directed separation of performance and fairness analyses. Worktree: `research/logp-analysis`; implementation baseline: `56a02ab`.

## Deliverable

Produce a self-contained paper-style analysis section, an implementation evidence appendix covering every DLock2 target and the legacy DLock1 API, and a deterministic executable mathematical-check artifact. The section must provide results and proofs, not only an experiment proposal.

## Scope and contracts

- Do not change production lock implementations or quietly repair away counterexamples.
- Separate source-level operation counts, an abstract communication machine, conditional mathematical theorems, and hardware performance estimates.
- Define publication, policy visibility, service, completion, and caller return separately.
- Count observed completions per pass rather than treating a source budget as batch size.
- Preserve the distinction between billed scheduler credit and useful protected service.
- Model fair/unfair differences within lock families; do not attribute all handoff migration to fairness.
- Cover all 21 DLock2 enum targets and map every DLock1 target, including dispatch limitations. Code advertised but not runnable must not be presented as measured or functioning.
- State the calibration and progress assumptions that theorems do not discharge. No hardware performance or whole-Rust correctness claim follows from a Python model check.
- User-approved revision: performance and fairness have separate models, assumptions, results, implementation comparisons, and validation criteria. Performance treats service policy and operation mix as inputs, without assuming a fairness theorem. Fairness proves service/selection properties without LogP parameters. Only a final discussion connects their outputs under additional explicit assumptions.

## Work

1. Audit the implementation inventory and complete the source evidence matrix.
2. Formalize a typed communication-event DAG and trace accounting, with explicit topology/resource and execution-epoch assumptions.
3. Derive migration amortization, throughput crossover, batch/return-delay feasibility, approximate-min fairness, credit-error sensitivity, useful-service charging bias, service-volume waiting, and announcement-visibility results.
4. Instantiate the model for the actual locks and give concrete counterexamples where a tempting stronger claim fails.
5. Build exact-rational and finite-state checks, with deterministic machine-readable results and no external Python dependencies.
6. Write and typeset the paper section and evidence appendix; independently review mathematical and source-grounding claims.
7. Run the artifact checks and PDF build, inspect layout, and document precisely what was and was not validated.

## Acceptance boundary

Theoretical-section readiness means defined assumptions, stated results with complete arguments, source-refined lock comparisons, reproducible mathematical checks, and honest limitations. It does not mean the full systems paper is submission-ready: independently calibrated hardware costs and held-out performance evaluation remain necessary before making quantitative systems claims. We will not disguise synthetic examples as machine measurements or assert novelty for classical batching/fair-scheduling facts.

## Artifacts

- `analysis/logp/paper.tex`: standalone typesetting driver and bibliography.
- `analysis/logp/section.tex`: shared scope and final discussion, including the two analyses.
- `analysis/logp/performance.tex`: communication, migration, throughput, return latency, and performance comparisons.
- `analysis/logp/fairness.tex`: service metrics, fairness proofs, visibility, accounting, and fairness comparisons.
- `analysis/logp/implementation-audit.tex`: implementation-specific evidence and coverage appendix.
- `analysis/logp/check_model.py`: executable finite checks and counterexample witnesses.
- `analysis/logp/README.md`: reproduction commands and claim/evidence boundary.
- `.worktree/logp/`: generated PDF, logs, and verification report (not source-controlled).

## Completion evidence

- Delivered separate performance and fairness models, theorem proofs, counterexamples, family comparisons, and a complete target inventory in a 14-page manuscript.
- Independently reviewed the mathematical claims and source-grounded inventory; retained explicit visibility, accounting, progress, and correctness limitations.
- All eight finite-state/exact-rational check groups passed in both standard Python and `python3 -O`; `cmp` confirmed byte-identical JSON reports.
- Built the revised PDF with `latexmk` and visually inspected all 14 pages. No undefined references or overfull boxes remain; the package-compatibility warning is recorded in the artifact README.
- Production lock implementations and environment configuration are unchanged. No Rust concurrency tests, hardware benchmarks, or empirical calibration are claimed.
- Completed the user-directed split into `performance.tex` and `fairness.tex`, with separate assumptions, comparison tables, and validation criteria; only the final discussion relates their outputs.
- Independent review found no material cross-dependency. Re-ran both Python modes after separation; all eight groups passed and the reports remained byte-identical.

## User-approved paper-story draft

Status: completed draft deliverable. The user requested a full narrative draft with explicit placeholders, following the discussion of performance modeling, conditional fairness/execution coupling, delegation, and FC-PQ. Missing research evidence remains explicitly unfinished.

- Preserve the verified separate-analysis manuscript. Add a companion `analysis/logp/story-draft.tex` with an evaluation input `story-evaluation.tex`.
- Write substantive English paper prose, not merely an outline: motivation and literature gap, performance model, independent fairness contract, conditional caller-execution tradeoff, delegation's architectural role, fairness cost, implementation, evaluation, and limitations.
- Use visible, stable placeholder IDs for missing citations/novelty assessment, calibration, protocol prediction, implementation refinement, plots, applications, and empirical results. Placeholders must state what evidence is needed; no fabricated results or repaired implementation behavior.
- Include the two-client execution-residency argument as an explicitly conditional model result with its proof. Do not claim the existing eight-check artifact verifies this new argument.
- This approval is for drafting and typesetting only. Production fixes, new benchmark instrumentation, simulator implementation, and execution of the proposed evaluation are not part of this task.
- Build and visually inspect the companion PDF; retain the original analysis PDF and its evidence boundary.

### Companion completion evidence

- Delivered `story-draft.tex` and `story-evaluation.tex`, with 21 uniquely identified placeholders, including four figure slots, and a claim/evidence map.
- Built the companion PDF with `latexmk` and visually inspected all eight pages. No undefined references or overfull boxes remain; the known compatibility warning and one underfull paragraph are recorded in the README.
- Independent mathematical review found no material error in the scoped proofs. Narrative review identified two control/notation issues, both corrected and rechecked: direct selector costs use identical state snapshots, and return-tail bounds use per-pass counts rather than average occupancy.
- Existing theory sources, check runner, production code, and environment configuration were preserved. No new finite-check execution, simulator, implementation repair, calibration, or benchmark result is claimed.

## Reference-paper narrative study (2026-09-24)

Status: completed primary-source reading and proposed story revision, not a
manuscript rewrite or approval for implementation/evaluation changes. The
existing TeX sources and generated PDFs remain unchanged by this study.

### Lessons from the reference papers

- [TCLocks, OSDI 2023](https://www.usenix.org/conference/osdi23/presentation/gupta):
  the opening distinguishes the known locality benefit of delegation from
  the obstacle to adopting it through ordinary lock APIs. Its contribution
  is transparent delegation, developed through concrete execution/stack and
  kernel-semantics challenges. The evaluation separates effectiveness from
  design-choice evidence. We should borrow this problem-to-obstacle structure,
  not present delegation's locality principle as our discovery.
- [CFL, PPoPP 2024](https://doi.org/10.1145/3627535.3638477):
  the motivating blogging/batch example shows why CPU and cgroup isolation
  do not schedule a shared lock. Requirements precede its virtual-hold-time,
  group, and NUMA-aware scheduling mechanisms. Application protection and
  aggregate throughput are distinct evaluation claims. Its reported throughput
  improvements rule out using it to support a universal fairness slowdown;
  optional strict-guarantee costs concern a narrower comparison.
- [SCL, EuroSys 2020](https://research.cs.wisc.edu/wind/Publications/eurosys20-scl.pdf):
  scheduler subversion is made concrete with UpScaleDB, then a toy example
  distinguishes acquisition fairness from usage/opportunity fairness.
  Accounting, penalties, and slices follow from explicit goals; evaluation
  returns to the opening application. Its lock-opportunity metric includes
  idle opportunity and is not automatically our protected-service metric.
  Section 7 already suggests usage-fair scheduling of delegated requests.
  Merely combining fairness and delegation is therefore insufficient novelty.

### Proposed argument

Lead with the question: what does a specified service-fairness guarantee cost,
and which costs depend on coupling the served requester to the executing core?
The model is the analytical means of answering that question, not an unexplained
opening contribution. Preserve independent performance and fairness analyses.

1. Establish the concrete service-allocation failure for heterogeneous work.
   Keep the motivating experiment a placeholder until measured.
2. Show requester identity, executor identity, and protected-state access in
   a small caller-execution/delegation diagram. State known prior principles.
3. Introduce the performance model to distinguish accounting/selection work,
   request communication, protected-state movement, and batch/return effects.
4. Define and prove the separate fairness contract, including eligible cohort,
   service metric, and accounting assumptions. Current FC-PQ refinement gaps
   remain explicit rather than becoming headline implementation guarantees.
5. Compose the analyses: present the residency result as conditional, and
   distinguish incremental policy cost from total modeled time and advantage
   over handoff. Reordering requests need not change the executor, but can
   still change cache behavior, operation mix, batching, and return latency.
6. Use FC-PQ as the implementation case study. Tie each mechanism and remaining
   obligation to the claimed contract rather than presenting a variant catalog.
7. Organize evaluation by claims: service guarantee, matched incremental cost,
   held-out performance prediction, causal attribution, and failure regimes.
   Return to the motivating workload to close the argument.

The potential contribution is a precise, supported separation of fairness
costs and execution-placement costs, with useful predictions or design
consequences. Novelty still needs broader related-work assessment; this
three-paper study does not certify it. No hardware result, new algorithm,
implementation repair, or new mathematical-check execution is claimed.

## User-approved simpler story revision

Status: completed revised draft deliverable. The user requested a new, simpler
draft based on the reference-paper study and the conditional comparison against
CFL. This approved the narrative rewrite, not production or experiment changes.

- Rewrite the existing companion around fair service with local execution.
  Start with heterogeneous service and the requester/executor distinction;
  make CFL the principal fair caller-executed performance comparison.
- Keep only a small cost model and its crossover in the main text. Explain
  the fairness contract in prose; move proofs, full accounting, implementation
  caveats, and evidence status into a technical appendix.
- Preserve substantive paper prose, correct citations to TCLocks/CFL/SCL,
  independent performance and fairness reasoning, and explicit result slots.
  Do not imply unconditional superiority, automatic residency, completed
  measurements, or unconditional fairness of current FC-PQ.
- Simplify the evaluation presentation around reader questions while retaining
  matched-work/resource controls and the distinction between prediction and
  trace reconstruction. Preserve the original separate-analysis manuscript.
- Build the revised companion, review its claims, inspect every PDF page,
  and update the artifact entry points and completion record.

### Simpler revision completion evidence

- Rewrote the companion as **Fair Service, Local Execution: Usage-Fair
  Delegation Locks**, starting from heterogeneous service and the distinction
  between served client and executing core.
- Made CFL the principal comparison, with a compact per-completion cost model
  and a conditional crossover. All scheduler/request costs remain included;
  CFL's NUMA-aware locality is credited rather than charging every handoff
  as cross-socket movement.
- Added `story-details.tex` for full accounting, the independent service-spread
  proof, the restricted residency proof, and implementation/evidence obligations.
  Main text contains only two numbered equation displays and an execution diagram.
- Reduced evaluation to four reader questions. Consolidated the former 21
  placeholders into 10 current slots, including one result-figure placeholder;
  no measurement was invented or silently completed.
- Built the six-page companion (four main/references, two appendix), and visually
  inspected all pages. No undefined references, overfull boxes, or underfull
  boxes remain; the existing `showhyphens` compatibility warning persists.
- Independent claim and narrative reviews completed. Integrated the advice to
  keep detailed measurement controls in the appendix and corrected TCLocks
  coauthor attribution against the official USENIX publication.
- Original analyses/checker, production code, and environment configuration
  unchanged. No new mathematical-check execution or empirical result is claimed.

## User-requested analysis-led narrative revision

Status: completed revised draft deliverable. The user requested **current practice →
state of the art → unresolved problem → analysis framework → solution based
on the analysis**, while retaining the simpler reader-facing presentation.

- Introduce shared locks as schedulers of serialized work. Present SCL/CFL
  and combining/TCLocks as established approaches, not new discoveries.
- State the missing design question before advocating FC-PQ: which costs
  come from service allocation, and which from execution placement?
- Present independent service and performance analyses, then explain the
  conditional connection that motivates request scheduling with retained
  execution. Describe FC-PQ as an existing concrete design interpreted through
  that analysis, not an implementation newly invented by this revision.
- Preserve the compact CFL comparison, matched-delegation cost distinction,
  evidence placeholders, and technical appendix. No changes to evaluation
  strategy, production code, original analyses, or experiments are authorized.
- Review source-grounded positioning and the problem-to-analysis-to-solution
  argument, build and visually inspect the revised PDF, and update records.

### Analysis-led revision completion evidence

- Reordered the companion around current practice and established work, the
  unresolved service/execution cost question, independent analytical tracks,
  their restricted connection, and a resulting FC-PQ design rationale.
- Made the analytical outputs explicit: the bounded-service-gap consequence
  for caller execution, a conditional CFL crossover, and accounting,
  admission, and batch-control obligations. Credited SCL's fair-delegation
  suggestion before stating the research gap; no claim of inventing the
  existing design or certifying broader novelty.
- Independent narrative and claim reviews completed; addressed all material
  findings. The connection states positive increments, persistent credits,
  and a gap bounded at service boundaries, rather than inheriting the
  service-spread theorem's allowance for zero increments.
- Built and visually inspected all six pages (four main/references, two
  appendix). Retained two main equation displays and 10 unique evidence
  slots. No undefined references or overfull/underfull boxes; the known
  `showhyphens` warning remains.
- Evaluation strategy/text, appendix, original analyses/checker, production
  code, and environment configuration unchanged. No experiments, calibration,
  new mathematical-check execution, or implementation repairs performed.

### Follow-up: introduce delegation after the usage-fair-lock problem

Status: completed. The user requested a later introduction of delegation.
Revise the abstract and introduction so usage-aware owner scheduling and its
remaining service/executor coupling are established first. Introduce delegation
as the analysis-motivated response, and move its broader prior-work context to
the design section. Preserve credit for CFL's locality policy and SCL's prior
fair-delegation suggestion; change neither mathematics nor evaluation strategy.
Build and inspect the PDF, then update the artifact records.

Completed by revising both the abstract and introduction: usage-fair owner
scheduling now precedes the remaining execution-coupling problem, the framework,
and the delegation response. Combining/TCLocks context and SCL's prior
fair-delegation suggestion moved to the design section. Built and visually
inspected all six pages; 10 unique evidence slots retained, with no undefined
references or overfull/underfull boxes. The known `showhyphens` warning remains.
No new independent review, mathematical checks, experiments, or production
changes were needed or claimed for this prose-only follow-up.

### User-requested SVG concept illustrations

Status: completed illustration revision. Approved scope:

- A short/long-operation service timeline explaining why equal turns need
  not mean equal protected service. No delegation in this opening figure.
- A replacement for the small execution table: identical service order under
  caller execution versus a retained combiner. Show remaining request traffic,
  not a claim of free communication or permanent cache residency.
- A schematic of the conditional cost crossover, including the per-request
  cost floor and the regime where batching cannot yield a strict advantage.

Use a consistent, labeled palette and legible vector graphics. Mark examples
as conceptual, preserve the problem-first narrative and all empirical slots,
and do not imply measured timings or a guaranteed hardware ranking. Keep SVG
sources with the manuscript and reproducibly export vector PDFs into the
ignored worktree artifact area. Build and visually inspect the figures and
the integrated paper. Mathematics, evaluation strategy, and production code
remain outside this illustration-only change.

The user also permitted Typst if easier. Retain LaTeX for this revision:
the SVG-to-vector-PDF export path is working, so migrating the existing
manuscript and appendix is unnecessary. SVG sources remain format-independent.

#### Illustration completion evidence

- Added three original, editable SVG sources under `analysis/logp/figures/`:
  service allocation, execution placement, and the conditional cost crossover.
  They are explicitly conceptual, with no empirical timings or speedups.
- Integrated the service figure in the introduction, the executor comparison
  in the design section, and the crossover figure with the cost condition.
  The problem-first narrative and all ten unique evidence slots are retained.
- Added `analysis/logp/render_figures.py`, exporting vector PDFs with librsvg.
  Exercised librsvg 2.61.4 from both the worktree and `/tmp`; generated assets
  remain in the ignored `.worktree/logp/figures/` directory.
- Built the seven-page companion: five main/references pages and two appendix
  pages. Visually inspected all three SVGs and all seven PDF pages.
  Balanced the final reference page; the other six rendered pages remained
  byte-identical after this layout adjustment. `pdfimages -list` confirms
  the final manuscript contains no raster images.
- Final log has no undefined references or horizontal overfull/underfull
  boxes. The known `showhyphens` warning remains; reference-page balancing
  adds a non-clipping 1.6873pt vertical-box warning.
- Updated artifact/reproduction documentation and completion tracking.
  Mathematics, evaluation strategy, technical appendix, original analyses,
  production code, and environment configuration are unchanged. No new
  independent review, model-check execution, calibration, or experiment
  was performed or claimed for this illustration revision.

### User-requested illustration-first HTML explainer

Status: completed. The user explicitly prioritized storytelling and ease
of understanding over paper format or length, and permitted HTML. Added an
offline, responsive HTML companion while preserving the LaTeX sources.

Approved presentation scope:
- Build a visual sequence from serialized capacity and unequal service,
  through usage-aware caller execution and independent analyses, to
  delegation, FC-PQ, conditional costs, and remaining evidence.
- Add explanatory SVGs for owner/executor coupling, the two analyses,
  eligibility, bounded caller runs, the combiner cycle, overlap, actual
  batch occupancy, completion versus return, residency limits, and the
  two distinct performance comparisons.
- Add small, deterministic teaching interactions for cumulative service,
  executor placement, and the cost crossover. These use explicit toy
  assumptions and arbitrary cost units, not measured results or a simulator
  of the Rust implementation.
- Keep the entire story readable without JavaScript; use native controls,
  textual alternatives, responsive layouts, and print styles. Load only
  local assets, without a build step, framework, CDN, or tracking.
- Preserve the ten missing-evidence identifiers, prior-work attribution,
  independent analytical premises, and current implementation caveats.
  Do not change mathematics, production locks, or evaluation strategy.
- Exercise the controls in a real browser, inspect desktop/mobile renderings,
  check offline/no-script reading and print output, and record exactly the
  checks performed.

#### HTML explainer completion evidence

- Delivered `analysis/logp/story-explainer.html`, with local CSS/JavaScript,
  twelve chapters, fifteen numbered visuals, and three optional teaching
  controls. Added ten SVGs under `analysis/logp/figures/explainer/`, reusing
  the three manuscript figures without modifying them.
- Maintained the problem-first sequence, late delegation introduction,
  independent service/execution premises, conditional CFL comparison,
  prior-work attribution, and all ten open evidence identifiers.
- Independent source reviews covered analytical/narrative fidelity and
  control behavior. The only reported material issue was no-JavaScript
  printing of closed technical notes; a CSS print fallback corrected it.
  Visual review also repaired missing intermediate SVG arrowheads, two
  tight label boxes, and print fragmentation of responsibility/failure cards.
- Exercised Chrome 147.0.7727.101 through Playwright: offline `file://`
  loading, native keyboard controls, all three teaching interactions,
  resets/limits, service costs 1–6, cost ties/wins/losses, zero costs,
  no-win floors, and fractional occupancy. No JavaScript errors or external
  requests occurred; all local assets/links and fragment targets resolved.
- Inspected all chapter renderings, individual new SVGs, desktop/mobile
  viewports, and mobile controls. Checked 390px and 320px document widths;
  diagrams scroll inside their containers rather than overflowing the page.
  Browser-measured text bounds fit all ten new SVG viewBoxes.
- Exercised the no-JavaScript reading path and browser print export.
  Inspected the final 24-page print layout; extracted no-script print text
  retained all ten evidence identifiers and expanded technical content.
  Reports, previews, and print exports live under the ignored
  `.worktree/logp/html-review/` directory.
- Updated README, index, and TODO. Production locks, environment files,
  mathematics, evaluation strategy, existing LaTeX manuscripts, and the
  original mathematical check runner are unchanged. UI smoke checks do not
  discharge any empirical, analytical-check, or implementation evidence slot.

### User-requested trilemma framing (2026-09-25)

Status: completed. Clarify the HTML explainer's motivation as a tension
between usage fairness, performance, and work conservation, before introducing
delegation. This is a presentation follow-up, not a new impossibility theorem
or a change to the research/evaluation plan.

- Define fairness over a stated cohort and metric, performance as useful-work
  throughput under matched work/resources, and work conservation against
  otherwise runnable demand rather than a policy-filtered eligible set.
- Explain the conditional costs of local-owner favoritism, service-driven
  handoff, and withholding service. Throttling one caller is not necessarily
  system idle; idling neither improves useful throughput nor erases a
  cumulative service gap.
- Distinguish fixed-backlog guarantees from lifetime equality involving
  inactive clients. Delegation changes owner/executor coupling, not demand
  availability or the need for admission rules.
- Integrate the response and limits into later HTML chapters. Preserve all
  existing mathematical claims, LaTeX sources, teaching controls, production
  code, and evidence slots.
- Exercise browser reading/interaction paths and inspect the affected
  desktop/mobile and print layouts before recording completion.

Completion: revised the opening, owner-scheduling chapter, framework,
delegation response, cost assumptions, and conclusion. Reused existing
responsive card styles; no CSS, JavaScript, SVG, LaTeX, or production changes.
Chrome 147 offline checks passed, including all three controls and the new
no-script disclosure. Inspected desktop/mobile cards and the 26-page print
layout. Preserved the ten evidence slots. No new independent analytical
review, mathematical-check execution, or empirical result is claimed.

### User-requested background and existing solutions (2026-09-25)

Status: complete. Expand the HTML companion's background and add a
mechanism-level existing-solutions chapter before the trilemma. The approved
follow-up is explanatory, not a new algorithm or evaluation direction.

- Explain critical sections, caller/owner/waiter roles, CPU versus lock
  scheduling, spinning versus blocking, and order versus service fairness.
- Describe FIFO queues and locality-aware handoff, then SCL accounting,
  penalties, and slices, and CFL virtual-hold-time queue reordering.
- Ground SCL in its primary paper §§2–4 and CFL in its primary paper
  §§3–3.4 and §4.6.1. Distinguish SCL opportunity from useful service and
  CFL's default work conservation from optional strict-guarantee idling
  and grace-period locality bias. Do not equate CFL with global-min selection
  or claim these paper options are enabled in this checkout.
- Preserve late delegation introduction, the conditional trilemma, all
  evidence identifiers, mathematics, LaTeX sources, and production code.
- Reuse existing responsive layouts; verify navigation, no-script reading,
  controls, and desktop/mobile/print presentation.

Completion: expanded the background and added the existing-solutions chapter,
bringing the companion to thirteen chapters. Mechanism cards explain SCL and
CFL before the trilemma. Chrome 147 offline checks passed for navigation,
all three controls, mobile overflow, native disclosures without JavaScript,
and print export. Inspected the revised desktop/mobile sections and the
31-page print layout, including printed SCL/CFL technical notes. No-script
print text retains those notes and all ten evidence identifiers. Production
files remain unchanged. No new independent analytical review, mathematical
check execution, or empirical result is claimed.
