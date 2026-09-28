#!/usr/bin/env python3
"""Finite abstract LogP sanity checks; neither hardware nor Rust verification."""

import argparse
from collections import deque
from fractions import Fraction
from itertools import product
import json
from math import lcm
from pathlib import Path
import sys


SCHEDULE = "abstract/source-inspired schedule; not an executed Rust trace"


class CheckFailure(Exception):
    """A model assertion failed (also active under python -O)."""


def require(condition, context):
    if not condition:
        raise CheckFailure(context)


def spread(values):
    return max(values) - min(values)


def normalized_states(n, bound):
    for values in product(range(bound + 1), repeat=n):
        if min(values) == 0:
            yield values


def spread_check():
    cases = 0
    # D0 may be smaller than the inductive bound: inspect the entire closed
    # invariant region, not just possible initial states.
    for n, d0, delta, cmax in product((2, 3, 4), (0, 1, 3),
                                       (0, 1, 2), (0, 1, 3)):
        limit = max(d0, delta + cmax)
        for usage in normalized_states(n, limit):
            for selected in range(n):
                if usage[selected] > delta:
                    continue
                for cost in range(cmax + 1):
                    updated = list(usage)
                    updated[selected] += cost
                    require(spread(updated) <= limit,
                            f"spread: {usage=}, {selected=}, {cost=}, {limit=}")
                    cases += 1
    return cases, {
        "kind": SCHEDULE, "assumptions": "fixed, continuously eligible cohort; "
        "no credit reset; nonnegative integer charges bounded by Cmax",
        "exact_min": {"before": [0, 0], "selected": 0, "charge": 3,
                      "after": [3, 0], "bound": 3},
        "approximate_min": {"before": [0, 2], "delta": 2,
                            "selected": 1, "charge": 3, "after": [0, 5],
                            "bound": 5},
    }


def accounting_error_check():
    cases = 0
    for n, d0, error, delta, cmax in product((2, 3), (0, 1, 2),
                                               (0, 1, 2), (0, 1, 2),
                                               (0, 1, 2)):
        limit = max(d0, error + delta + cmax)
        errors = tuple(normalized_states(n, error))
        for usage in normalized_states(n, limit):
            for offsets in errors:
                # V = U + e; translating e does not change eligibility.
                virtual = tuple(u + e for u, e in zip(usage, offsets))
                minimum = min(virtual)
                for selected in range(n):
                    if virtual[selected] > minimum + delta:
                        continue
                    for cost in range(cmax + 1):
                        updated = list(usage)
                        updated[selected] += cost
                        require(spread(updated) <= limit,
                                f"accounting_error: {usage=}, {offsets=}, "
                                f"{selected=}, {cost=}, {limit=}")
                        cases += 1
    return cases, {
        "kind": SCHEDULE, "assumptions": "fixed cohort; each selection uses "
        "V_i=U_i+e_i with pairwise error range at most E; errors may vary "
        "between selections; bounded nonnegative true charge",
        "before_U": [0, 3], "errors": [2, 0], "V": [2, 3],
        "delta": 1, "E": 2, "selected": 1, "charge": 2,
        "after_U": [0, 5], "bound": 5,
    }


def waiting_bound_check():
    cases = 0
    terminal = 0
    # The target (index 0) never accrues charge until chosen. Enumerate all
    # eligible ties and all competitor charge choices, merging identical states.
    for n, initial, delta, cmin in (
        (n, initial, delta, cmin)
        for n in (2, 3)
        for initial in product(range(3), repeat=n)
        for delta in range(3)
        for cmin in (1, 2)
    ):
        bounds = tuple(max(0, (initial[0] + delta - initial[j]) // cmin + 1)
                       for j in range(1, n))
        first = (initial, (0,) * (n - 1))
        pending = [first]
        seen = {first}
        terminated_here = False
        while pending:
            usage, counts = pending.pop()
            minimum = min(usage)
            for chosen in range(n):
                if usage[chosen] > minimum + delta:
                    continue
                if chosen == 0:
                    terminal += 1
                    cases += 1
                    terminated_here = True
                    continue  # Stop this trace at the target's first selection.
                for cost in range(cmin, 4):
                    updated_counts = list(counts)
                    updated_counts[chosen - 1] += 1
                    require(updated_counts[chosen - 1] <= bounds[chosen - 1],
                            f"waiting_bound: {initial=}, {usage=}, "
                            f"{chosen=}, {cost=}, {delta=}, {cmin=}")
                    updated_usage = list(usage)
                    updated_usage[chosen] += cost
                    state = (tuple(updated_usage), tuple(updated_counts))
                    if state not in seen:
                        seen.add(state)
                        pending.append(state)
                    cases += 1
        require(terminated_here, f"target never selected: {initial=}, {delta=}")
    return cases, {
        "kind": SCHEDULE, "assumptions": "fixed, continuously visible "
        "target and competitors, no resets, every competitor charge >= cmin > 0; "
        "all approximate-min ties allowed; target stops on first selection",
        "initial_U": [2, 0], "delta": 0, "cmin": 1,
        "competitor_sequence": [0, 1, 2, 3],
        "selections_before_target": 3, "competitor_bound": 3,
        "target_first_selection_at_tie_or_after": True,
        "terminal_target_choices_checked": terminal,
    }


def crossover_check():
    cases = 0
    no_benefit = 0
    unit_batch = 0
    grid = (Fraction(0), Fraction(1, 2), Fraction(1), Fraction(2))
    for d, a, m, k, h in product(grid, repeat=5):
        numerator = a + m + k
        denominator = m + h - d
        for batch in (1, 2, 3, 4, 8):
            delegation = d + numerator / batch
            handoff = h + m
            wins = delegation < handoff
            require(wins == (batch * denominator > numerator),
                    f"crossover: {d=}, {a=}, {m=}, {k=}, {h=}, {batch=}")
            if denominator <= 0:
                require(not wins, "nonpositive crossover denominator won")
                no_benefit += 1
            if batch == 1:
                unit_batch += 1
            cases += 1
    return cases, {
        "assumptions": "conditional matched-work serialized-path approximation; "
        "nonnegative A,M,K,d,h, p=1, no exposed idle term; exact rational costs",
        "strict_inequality": "d+(A+M+K)/b < h+M iff b*(M+h-d)>A+M+K",
        "nonpositive_denominator_cases": no_benefit,
        "batch_one_cases": unit_batch,
        "batch_one_can_win": {"d": "0", "A": "0", "M": "0", "K": "0",
                              "h": "1", "b": 1},
    }


def batch_frontier_check():
    cases = 0
    feasible_regions = 0
    grid = (Fraction(0), Fraction(1, 2), Fraction(2))
    for d, a, m, k, h in product(grid, repeat=5):
        numerator = a + m + k
        denominator = m + h - d
        bmin = numerator // denominator + 1 if denominator > 0 else None
        for qmax, etail, slack in product((Fraction(1, 2), Fraction(2)),
                                          (Fraction(0), Fraction(1), Fraction(2)),
                                          (Fraction(0), Fraction(1, 2),
                                           Fraction(1), Fraction(3))):
            budget = etail + slack  # D >= Etail, qmax > 0.
            bmax = 1 + (budget - etail) // qmax
            brute = [b for b in range(1, bmax + 1)
                     if d + numerator / b < h + m
                     and etail + (b - 1) * qmax <= budget]
            algebra = list(range(bmin, bmax + 1)) if bmin is not None else []
            require(brute == algebra,
                    f"batch_frontier: {d=}, {a=}, {m=}, {k=}, {h=}, "
                    f"{qmax=}, {etail=}, {budget=}, {brute=}, {algebra=}")
            feasible_regions += bool(brute)
            cases += 1
    return cases, {
        "assumptions": "conditional matched-work serialized-path model as in "
        "crossover, plus deterministic tail model Etail+(b-1)*qmax <= D; "
        "qmax>0, D>=Etail, b is a positive integer; None when no "
        "positive crossover denominator",
        "b_min": "floor((A+M+K)/(M+h-d))+1 for M+h-d>0",
        "b_max": "1+floor((D-Etail)/qmax)",
        "regions_with_feasible_batch": feasible_regions,
        "illustration": {"A": "2", "M": "0", "K": "0", "h": "2",
                         "d": "0", "Etail": "1", "D": "3",
                         "qmax": "1", "b_min": 2, "b_max": 3,
                         "feasible_batches": [2, 3]},
    }


def charge_bias_check():
    cases = 0
    for c1, c2, theta1, theta2 in product(range(1, 5), range(1, 5),
                                          range(4), range(4)):
        costs = (c1, c2)
        charged = (c1 + theta1, c2 + theta2)
        common = lcm(*charged)
        operations = tuple(common // value for value in charged)
        require(all(n * value == common for n, value in zip(operations, charged)),
                "cycle failed to equalize charged service")
        useful = tuple(n * c for n, c in zip(operations, costs))
        actual = tuple(Fraction(value, sum(useful)) for value in useful)
        ratios = tuple(Fraction(c, charge) for c, charge in zip(costs, charged))
        expected = tuple(value / sum(ratios) for value in ratios)
        require(actual == expected,
                f"charge_bias: {costs=}, {charged=}, {operations=}")
        cases += 1
    return cases, {
        "kind": SCHEDULE, "assumptions": "saturated complete cycles with "
        "fixed per-operation useful cost c_i>0 and fixed charged overhead "
        "theta_i>=0; equal charged service, not equal operation counts",
        "costs": [1, 3], "overheads": [1, 0],
        "equal_charged_service_per_class": 6,
        "operations": [3, 2], "useful_service": [3, 6],
        "useful_shares": ["1/3", "2/3"],
    }


def announcement_visibility_check():
    cases = 0
    for capacity, budget in product(range(1, 6), range(1, 7)):
        for earlier in range(3 * capacity + 2):
            for remaining_current in range(budget + 1):
                # Arrival is *after* this pass's drain. Its still-pending
                # service may contain any number up to H completions.
                queue = deque(range(earlier))
                queue.append("target")
                intervening = remaining_current
                passes = 0
                while True:
                    passes += 1
                    drained = [queue.popleft() for _ in range(min(capacity, len(queue)))]
                    if "target" in drained:
                        require(drained[-1] == "target", "FIFO drain reordered tickets")
                        break
                    intervening += budget  # Worst case: H completions before next drain.
                max_passes = (earlier + capacity) // capacity
                bound = budget * max_passes
                require(passes == max_passes and intervening <= bound,
                        f"announcement_visibility: {capacity=}, {budget=}, "
                        f"{earlier=}, {remaining_current=}, {intervening=}")
                cases += 1
    return cases, {
        "kind": SCHEDULE, "assumptions": "published FIFO tickets; no stalled "
        "earlier reservation; every pass drains min(R,queued) tickets, "
        "then performs at most H delegate completions; passes continue; "
        "R is a drain quota, not a claim of unbounded physical ring capacity",
        "R": 2, "H": 3, "q": 4, "drain_passes_until_found": 3,
        "max_intervening_completions": 9,
        "bound": "H*ceil((q+1)/R)",
    }


def counterexamples_check():
    cases = 0
    # Source-inspired values only; the source does not execute this schedule.
    old = 1000
    newcomer = 2000 // 2000  # total charge / total operations, not virtual time.
    require(old - newcomer > 1, "newcomer initialization did not break Cmax")
    catchup = old - newcomer
    require(newcomer + catchup == old and catchup == 999
            and (catchup + 63) // 64 > 8,
            "newcomer catch-up witness is inconsistent")
    cases += 1

    popped, next_min, waited_passes = 1, 2, 9
    clamped = min(popped, next_min) if waited_passes > 8 else popped
    require(clamped == popped, "post-pop min clamp unexpectedly changed minimum")
    cases += 1

    visible, hidden = 0, 0
    for _ in range(2):
        visible += 1  # Hidden low-credit request is not in the local PQ.
    require(visible - hidden > 1, "hidden requester did not break Cmax")
    cases += 1

    head, tail, published = 0, 1, False  # Reserved slot, no valid payload.
    for wall_delay in (1, 1000, 1000000):
        require(head != tail and not published,
                f"unexpected drain progress after {wall_delay} wall ticks")
    cases += 1

    queue = deque([(0, False)])  # (credit, complete)
    buffer = []
    attempts = completions = 0
    for _ in range(64):
        attempts += 1
        if not queue:
            break
        credit, complete = queue.popleft()
        if complete:
            buffer.append((credit, complete))
        else:
            completions += 1
            queue.append((credit + 1, True))
    require(attempts == 3 and completions == 1 and len(buffer) == 1,
            "64 pop-attempt model did not terminate with fewer completions")
    cases += 1

    fifo_rate = Fraction(2, 1 + 9)  # One acquisition of each cost.
    usage_rate = Fraction(9 + 1, 9 * 1 + 1 * 9)  # Equal *useful* service.
    require(fifo_rate == Fraction(1, 5) and usage_rate == Fraction(5, 9)
            and fifo_rate != usage_rate, "operation mix rates coincide")
    cases += 1
    # A near-perfect final Jain index need not imply bounded absolute lag.
    jfi_values = []
    for scale in (10, 100, 1000):
        credits = (scale * scale, scale * scale + scale)
        jfi = Fraction(sum(credits) ** 2, 2 * sum(u * u for u in credits))
        require(spread(credits) == scale
                and jfi == 1 - Fraction(1, 4 * scale * scale + 4 * scale + 2),
                f"Jain index / lag witness failed at scale {scale}")
        jfi_values.append(str(jfi))
    require(jfi_values == ["441/442", "40401/40402", "4004001/4004002"],
            "Jain index did not approach one as absolute lag increased")
    cases += 1
    return cases, {
        "kind": SCHEDULE,
        "newcomer_mean_not_baseline": {"old_pending_credit": old,
            "historical_total_charge": 2000, "historical_operations": 2000,
            "newcomer_initial_credit": newcomer, "Cmax": 1,
            "initial_spread": 999,
            "consecutive_newcomer_unit_services_before_tie": catchup,
            "minimum_passes_at_64_pop_budget": (catchup + 63) // 64,
            "qualification": "only under repeated eligible resubmission, "
            "unserved old request, and favorable newcomer selection; 64 pop "
            "attempts may yield fewer completions; finite catch-up is not "
            "unconditional starvation"},
        "post_pop_min_clamp_inert": {"waited_passes": waited_passes,
            "popped_min": popped, "next_min": next_min, "after_clamp": clamped},
        "hidden_low_credit": {"initial_visible_hidden": [0, 0],
            "two_visible_unit_services": [2, 0], "Cmax": 1,
            "qualification": "hidden requester violates continuous visibility"},
        "reserved_without_publish": {"head": head, "tail": tail,
            "valid": published, "sample_wall_delays_without_progress": [1, 1000, 1000000],
            "qualification": "without a publication/progress assumption, no "
            "finite wall-clock bound follows"},
        "pop_attempts_not_completions": {"budget": 64,
            "attempts_before_empty": attempts, "completions": completions},
        "mix_throughput": {"operation_costs": [1, 9],
            "equal_acquisition_cycle_counts": [1, 1],
            "equal_acquisition_rate": str(fifo_rate),
            "equal_usage_cycle_counts": [9, 1],
            "equal_usage_rate": str(usage_rate)},
        "jain_index_not_lag_bound": {
            "scales": [10, 100, 1000], "absolute_lags": [10, 100, 1000],
            "final_jain_indices": jfi_values,
            "qualification": "finite examples illustrate unbounded-lag family "
            "(k^2,k^2+k) with Jain index tending to one; no inferred time bound"},
    }


CHECKS = (
    ("spread", spread_check),
    ("accounting_error", accounting_error_check),
    ("waiting_bound", waiting_bound_check),
    ("crossover", crossover_check),
    ("batch_frontier", batch_frontier_check),
    ("charge_bias", charge_bias_check),
    ("announcement_visibility", announcement_visibility_check),
    ("counterexamples", counterexamples_check),
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="JSON report path")
    args = parser.parse_args()
    report = {
        "artifact_version": 1,
        "scope": "Deterministic finite abstract/source-inspired LogP model checks; "
                 "not executed Rust traces, hardware measurement, or proof of Rust correctness",
        "checks": [], "examples": {},
    }
    try:
        for name, check in CHECKS:
            count, witness = check()
            require(isinstance(count, int) and count > 0,
                    f"{name}: no cases explored")
            report["checks"].append({"name": name, "cases": count,
                                     "status": "pass"})
            report["examples"][name] = witness
            print(f"{name}: pass ({count} cases)")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("w", encoding="utf-8") as stream:
            json.dump(report, stream, indent=2, sort_keys=True)
            stream.write("\n")
    except (CheckFailure, OSError) as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        return 1
    print("No hardware measurements or Rust verification; abstract/source-inspired "
          "schedules are not executed Rust traces.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
