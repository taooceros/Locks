# Evidence prune (2026-09-28)

Status: approved by the user (task assignment); docs only, no code.

## Goal

Remove completed results that no longer make sense under the 2026-09-28
decisions, and keep provenance for each one:

- waiters are spin-only (parking E0(a)/E0(a') shelved), so every experiment
  keeps threads <= CPUs; oversubscription is a stated limitation, not a result;
- new FC-PQ runs use the E0(b) `fcpq_fast_path` (PR #48, open), so the old
  1-worker "constant tax" claims are replaced by the E0(b) measurement;
- the external-lock redb results are invalid (redb's internal writer lock was
  never contended) and will be regenerated with the internal-lock harness;
- fairness is service-time share (Jain over per-thread service time).

## Changes

1. Delete the withdrawn results from `README.md`, `docs/evidence/README.md`,
   `docs/findings/002-*`, `docs/findings/003-*`, `docs/story-candidates.md`
   and the ledger copy `docs/evidence/all-experiments-2026-09-25.md`.
   Clean removal, no strike-through.
2. Add `## Withdrawn` to `docs/evidence/README.md`: one line per removed
   study with the reason and where it still lives (branch or
   `~/Locks-artifacts/<dir>`).
3. Replace the 1-worker tax claims with E0(b): tax 64.7 ns -> -8.1 ns,
   1W FC-PQ/FC 0.53 -> 1.12, saturated 32W still 0.65 (cs 1) / 0.88 (cs 1000).
4. Reword H5 and E3 (README, TODO) to spin-only, threads <= CPUs; redb is
   regenerated with the internal-lock harness.
5. Keep finding 002 as the rationale for the threads <= CPUs rule (mechanism
   only, numbers from oversubscribed runs removed).
6. Prune the ledger copy to the surviving studies (most sections survive);
   the verbatim original stays on `experiment/upscaledb-fc-pq-integration`.

Not touched: branches, bookmarks, `~/Locks-artifacts`, `docs/archive/`.

Follow-up approved by Main (same change): also withdraw the old `TODO.md` CFL
smoke-test note, the packed-2W, one_equal and single-worker DB ratios
(low-occupancy, pre-fast-path); make E4 4 + 4 on 8 CPUs; mark E0(a) shelved
(`uwqronmm`, E0(a') `zlqxmnyz` unmerged); record that PR #48 meets the E0(b)
1W target with default-off features.

## Acceptance

- No remaining citation of the withdrawn numbers outside the Withdrawn list.
- All relative links in the changed files resolve.
