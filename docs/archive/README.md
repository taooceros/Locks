# Archive

These documents predate the thesis refocus of 2026-09-26 and are kept for provenance only; the current thesis and research plan live in [`../../README.md`](../../README.md).

| File | What it was | Why archived | Still useful for |
|------|-------------|--------------|------------------|
| [RESEARCH_PLAN.md](RESEARCH_PLAN.md) | First research plan: "Usage-Fair Delegation Locks", problem statement (usage unfairness + combiner latency penalty), contributions, positioning vs CFL/Syncord/TCLocks/U-SCL, evaluation plan, paper outline | Broad multi-contribution framing superseded by the single-thesis README | Related-work positioning text, evaluation-plan ideas, paper outline skeleton |
| [RESEARCH_PLAN_2.md](RESEARCH_PLAN_2.md) | Second research plan draft: "Breaking the Fairness-Performance Tradeoff", core thesis that fair delegation avoids the handoff cost term, four RQs | Closest precursor of the current thesis; wording replaced by the verbatim thesis in README | RQ formulation, argument structure for the "dominant cost term" claim |
| [STATUS_REPORT.md](STATUS_REPORT.md) | Multi-agent code-review status report (2026-02-25): critical bugs (BUG-1..), blockers, algorithm improvements, venue strategy (PPoPP 2027 / EuroSys 2027) | Most bugs resolved; venue/strategy sections no longer drive the work | Commit hashes of bug fixes (e.g. `353f1ca`), list of unresolved issues such as `num_waiting_threads` overestimation |
| [EXPERIMENT_PLAN.md](EXPERIMENT_PLAN.md) | Full 10-group experiment spec (common parameters, machines, hypotheses, metrics, figure mapping) that `experiment.nu` was written against | Experiment matrix now derived from the README thesis and `docs/evidence/` | Machine specs (saturn), exact CLI parameters behind `experiment.nu` groups, figure-to-experiment mapping |
| [EXPERIMENT_RESULTS_DEMO.md](EXPERIMENT_RESULTS_DEMO.md) | Demo run (1s duration, 1 trial) of all 15 DLock2 locks: JFI/throughput tables per group | Demo-quality numbers; real evidence lives in `docs/evidence/` | Sanity-check magnitudes, lock grouping (delegation unfair/fair, traditional, C baselines) |
| [algorithm-improvement-plan-2026-03-23.md](algorithm-improvement-plan-2026-03-23.md) | Plan for algorithm directions beyond FC-PQ: FC-EW (eligibility window), combiner-budgeted combining, resumable/sliced operations | Thesis refocus fixes FC-PQ as the mainline design; extensions deferred | Future-work section, rationale for de-emphasising FC-SL and extra PQ variants |
| [proposal-hilldale.typ](proposal-hilldale.typ) | Hilldale fellowship proposal "Usage-Fairness in Delegation-Styled Locks" (Typst, uses `template.typ`) | Funding proposal, not a research document | Abstract wording, early motivation text |
| [proposal-st.typ](proposal-st.typ) | Same proposal reformatted for a second (ST) application | Duplicate of the Hilldale proposal with minor edits | Same as above |
| [template.typ](template.typ) | Typst page/heading template used by the two proposals above | Only needed to compile the archived proposals | Compiling the proposals |
| [meeting-2026-03-12.typ](meeting-2026-03-12.typ) | Advisor meeting notes 2026-03-12: stochastic combiner backoff, relaxed priority queue (SprayList), plain-publish balanced tree; weekly plan | Meeting notes | Origin of the FC-PQ tree idea and combiner-backoff idea |
| [meeting-2026-03-19.typ](meeting-2026-03-19.typ) | Advisor meeting notes 2026-03-19: lock list under test, synthetic workload design (two CS groups, NCS sweep) | Meeting notes | Early description of the synthetic benchmark design |
| [advisor-meeting.typ](advisor-meeting.typ) | Touying slide deck giving an overview of the Locks repository for an advisor meeting | Presentation, no longer matches the repository layout | Diagram/slide templates (cetz, fletcher) |
| [report.typ](report.typ) | Early (2023) Typst report on delegation locking: FC/CC-Synch/RCL/ffwd implementation notes, lock-slice comparison with U-SCL | Predates the fairness work; implementation details drifted | Background text on delegation locks and lock slices |
| [flatcombining.typ](flatcombining.typ) | cetz state-machine illustration of the Flat Combining thread loop, included by `report.typ` | Only used by the archived report | Reusable FC diagram |
| [report-literature.yml](report-literature.yml) | Hayagriva bibliography for `report.typ` (ccsynch, scl, ...) | Only used by the archived report; `docs/reference/literature.yml` is the live bibliography | Cross-checking entries |
| [ref.bib](ref.bib) | Single-entry BibTeX file (crossbeam GitHub) used by `report.typ` | Only used by the archived report | Nothing beyond the one entry |
| [proposal-early.typ](proposal-early.typ) | Earliest proposal draft: async/await-based user-level scheduler with usage-fair lock handling, motivation and abstract | Idea abandoned in favour of delegation-lock fairness | Record of the discarded scheduler direction |
| [proposal-early-literature.yml](proposal-early-literature.yml) | Hayagriva bibliography for `proposal-early.typ` | Only used by the archived early proposal | Cross-checking entries |
| [TODO-pre-thesis-2026-09-26.md](TODO-pre-thesis-2026-09-26.md) | Full pre-refocus TODO with 10 phases | Superseded by the README.md research plan | Commit hashes of completed infrastructure |

## Deleted (recoverable from git history)

- `docs/proposal/proposal-hilldale.pdf`, `docs/proposal/proposal-st.pdf` - generated artifacts (compiled Typst).
- `presentation/advisor-meeting.pdf` - generated artifact (compiled slides).
- `visualization/report/report.pdf`, `visualization/proposal/proposal.pdf` - generated artifacts (compiled Typst).
- `docs/related-work/CFL-PPoPP24.pdf`, `ShflLock-SOSP19.pdf`, `Syncord-OSDI22.pdf`, `TCLocks-OSDI23.pdf` - third-party PDFs; the `.txt` extractions remain in `docs/related-work/`.
- `profiles/` (perf `.stats`/`.txt` for every DLock2 lock) - regenerated by `profile.nu`; now gitignored.
- `visualization/graphs/` (16 SVGs) - generated plot artifacts.
- `visualization/notebooks/graph.jl` - superseded Julia plotting notebook; Python scripts in `visualization/` replace it.
- `visualization/.vscode/` - editor configuration (ltex dictionary).
- `visualization/report/test.yml` - scratch Hayagriva entry (TCLocks) unused by any document.
- `justfile.shell.nu` - empty file.
