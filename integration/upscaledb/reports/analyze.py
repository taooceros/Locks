#!/usr/bin/env python3
"""Analyze run_trials.py manifest + raw records without silently dropping failures.

  python3 -m integration.upscaledb.reports.analyze --input-dir .worktree/upscaledb/output
  python3 -m integration.upscaledb.reports.analyze --input-dir .worktree/upscaledb/output --output-dir /tmp/ups-analysis

Plots require matplotlib >= 3.8 (install with `python3 -m pip install 'matplotlib>=3.8'`
in the analysis Python environment; no matplotlib dependency for the runner). The exact
Python/matplotlib versions and backend are saved in summary.json. Bootstrap draws
are independent *trial summaries* (paired trial blocks for comparisons), never ops.
CIs need >=10 complete trials/paired blocks; smaller samples are exploratory only.
Native and bridge profiles are instrumentation controls, excluded from primary
rankings. Duration uses equal observation windows, before-deadline service and
operation-specific throughput; mixed total throughput can change its mix.
"""
import argparse
import base64
import binascii
import csv
import json
import math
from pathlib import Path
import random
import statistics
import sys

from integration.upscaledb.runner.run_trials import DEFAULT_OUTPUT, VARIANTS, digest, reject_nonfinite

PRIMARY = ('native', 'refactored', 'bridge_mutex', 'fc', 'fc_pq', 'uscl', 'cfl_local',
           'spinlock', 'mcs', 'ticket', 'clh')
PROFILES = ('profile', 'bridge_mutex_profile', 'fc_profile', 'fc_pq_profile',
            'uscl_profile', 'cfl_local_profile', 'spinlock_profile', 'mcs_profile',
            'ticket_profile', 'clh_profile')
PROFILE_BASE = {profile: 'native' if profile == 'profile' else profile.removesuffix('_profile')
                for profile in PROFILES}
BRIDGE = tuple(variant for variant in VARIANTS if variant not in ('native', 'refactored', 'profile'))
OP_NAMES = ('find', 'insert')
QUANTILES = {'p50': (50, 100, 2), 'p95': (95, 100, 20),
             'p99': (99, 100, 100), 'p99_9': (999, 1000, 1000)}
FLAGS = ('upstream_sha', 'environment', 'db_flags', 'key_type', 'key_bytes',
         'record_bytes', 'transactions', 'recovery', 'duplicates', 'compression',
         'direct_access', 'record_value_formula', 'insert_key_formula', 'clock',
         'histogram_label', 'memory_policy')


class BadTrial(ValueError):
    pass


def require(condition, reason):
    if not condition:
        raise BadTrial(reason)


def integer(value, label):
    require(type(value) is int and value >= 0, label + ': expected nonnegative integer')
    return value


def pair(value, label):
    require(type(value) is list and len(value) == 2, label + ': expected [find,insert]')
    return [integer(x, label) for x in value]

def jain(values):
    if len(values) < 2 or not any(values):
        return None
    return sum(values) ** 2 / (len(values) * sum(n * n for n in values))

def role_groups(cfg):
    all_workers = range(cfg['workers'])
    if cfg['inserters'] == 0:
        # One alternating worker belongs to BOTH operation types, not a
        # dedicated finder with a nonexistent inserter.
        return (('all', all_workers), ('mixed', all_workers))
    return (('all', all_workers), ('finder', range(cfg['finders'])),
            ('inserter', range(cfg['finders'], cfg['workers'])))


def histogram(obj, label):
    require(isinstance(obj, dict), label + ': missing histogram')
    bins = obj.get('bins')
    require(type(bins) is list and len(bins) == 64, label + ': expected 64 bins')
    bins = [integer(x, label + '.bin') for x in bins]
    samples = integer(obj.get('samples'), label + '.samples')
    require(sum(bins) == samples, label + ': bin sum differs from samples')
    total_ns = obj.get('sum_ns')
    if total_ns is not None:
        total_ns = integer(total_ns, label + '.sum_ns')
    if 'min_ns' in obj or 'max_ns' in obj:
        low, high = obj.get('min_ns'), obj.get('max_ns')
        require((samples == 0 and low is None and high is None) or
                (samples > 0 and type(low) is int and type(high) is int and
                 0 <= low <= high and (total_ns is None or low * samples <= total_ns <= high * samples)),
                label + ': min/max/sum inconsistent')
    minimums = obj.get('quantile_min_samples')
    if minimums is not None:
        require(isinstance(minimums, dict) and set(minimums) == set(QUANTILES),
                label + ': unknown quantile support')
        minimums = {key: integer(value, label + '.quantile_min_samples')
                    for key, value in minimums.items()}
        require(all(n > 0 for n in minimums.values()), label + ': quantile support must be positive')
    else:
        minimums = {key: spec[2] for key, spec in QUANTILES.items()}
    return bins, samples, total_ns, minimums


def quantile(bins, samples, name, minimums):
    numerator, denominator, _ = QUANTILES[name]
    if samples < minimums[name]:
        return None
    rank = (samples * numerator + denominator - 1) // denominator
    seen = 0
    for index, count in enumerate(bins):
        seen += count
        if seen >= rank:
            return None if index == 63 else (0 if index == 0 else 1 << index)
    raise BadTrial('histogram count/rank inconsistent')


def metrics_for_histograms(workers, category, operation):
    bins = [0] * 64
    samples = 0
    sums = []
    support = None
    for worker in workers:
        item = worker[category][operation] if category == 'latency_ns' else worker[
            'native_mutex_profile'][operation][category]
        counts, n, total, minimums = histogram(item, category + '.' + operation)
        require(support is None or support == minimums, 'quantile sample thresholds differ')
        support = minimums
        samples += n
        bins = [a + b for a, b in zip(bins, counts)]
        sums.append(total)
    return {'samples': samples, 'sum_ns': sum(sums) if all(x is not None for x in sums) else None,
            'mean_ns': sum(sums) / samples if samples and all(x is not None for x in sums) else None,
            'quantile_min_samples': support,
            'quantile_upper_bound_ns': {q: quantile(bins, samples, q, support) for q in QUANTILES}}


def metrics_for_histograms_bridge(workers, operation):
    groups = [histogram(w['bridge_profile']['service_ns'][operation],
                        'bridge.service_ns.' + operation) for w in workers]
    support = groups[0][3]
    require(all(g[3] == support for g in groups), 'bridge histogram thresholds differ')
    bins = [sum(g[0][i] for g in groups) for i in range(64)]
    samples = sum(g[1] for g in groups)
    total = sum(g[2] for g in groups)
    return {'samples': samples, 'sum_ns': total, 'mean_ns': total / samples if samples else None,
            'quantile_min_samples': support,
            'quantile_upper_bound_ns': {q: quantile(bins, samples, q, support) for q in QUANTILES}}


def bridge_metrics(result, workers, variant, completed, on_time, mode):
    """Validate request attribution against physical execution, without conflating units."""
    root = result.get('bridge_profile')
    require(isinstance(root, dict), 'missing bridge global counters')
    instrumented = variant in BRIDGE and variant in PROFILES
    bridge = variant in BRIDGE
    require(type(root.get('enabled')) is bool and root['enabled'] is instrumented,
            'bridge global instrumentation state mismatches variant')
    fields = ('accepted_calls', 'completed_calls', 'rejected_reentrant',
              'peak_inflight_calls', 'active_calls')
    global_counts = {key: integer(root.get(key), 'bridge_profile.' + key) for key in fields}
    requests = []
    service = []
    before = []
    tsc = []
    before_tsc = []
    executors = []
    for worker in workers:
        profile = worker.get('bridge_profile')
        require(isinstance(profile, dict) and type(profile.get('enabled')) is bool and
                profile['enabled'] is instrumented, 'bridge worker instrumentation state mismatch')
        count = pair(profile.get('requested_calls'), 'bridge.requested_calls')
        require(count == (worker['attempted'] if bridge else [0, 0]),
                'bridge requests differ from attempted operations')
        request_ns = profile.get('service_ns')
        require(isinstance(request_ns, dict), 'missing requester service histograms')
        sums = []
        for op_index, op in enumerate(OP_NAMES):
            _, samples, total, _ = histogram(request_ns.get(op), 'bridge.service_ns.' + op)
            require(samples == (count[op_index] if instrumented else 0),
                    'requester service samples differ from completed bridge requests')
            require(total is not None and (instrumented or total == 0),
                    'missing requester service sum or disabled metric not zero')
            sums.append(total)
        ticks = pair(profile.get('service_tsc_ticks'), 'bridge.service_tsc_ticks')
        early = pair(profile.get('service_before_deadline_ns'), 'bridge.service_before_deadline_ns')
        early_ticks = pair(profile.get('service_before_deadline_tsc_ticks'),
                           'bridge.service_before_deadline_tsc_ticks')
        require(all(e <= n and t <= s for e, n, t, s in zip(early, sums, early_ticks, ticks)),
                'before-deadline service exceeds full service')
        require(mode != 'fixed' or (early == sums and early_ticks == ticks),
                'fixed-work before-deadline service differs from full service')
        require(instrumented or not any(sums + ticks + early + early_ticks),
                'disabled bridge requester measurements are nonzero')
        executor = profile.get('executor')
        require(isinstance(executor, dict) and type(executor.get('enabled')) is bool and
                type(executor.get('has_combiner_pass_ticks')) is bool and
                executor['enabled'] is instrumented, 'physical executor state mismatch')
        values = {key: integer(executor.get(key), 'bridge.executor.' + key) for key in
                  ('executed_callbacks', 'executed_service_tsc_ticks', 'executed_service_ns',
                   'combiner_pass_tsc_ticks')}
        require(instrumented or not any(values.values()),
                'disabled physical executor measurements are nonzero')
        require(executor['has_combiner_pass_ticks'] or values['combiner_pass_tsc_ticks'] == 0,
                'unavailable combiner-pass counter nonzero')
        require(executor['has_combiner_pass_ticks'] is
                (variant in ('fc_profile', 'fc_pq_profile')),
                'combiner-pass availability differs from selected profile backend')
        if executor['has_combiner_pass_ticks']:
            require(values['combiner_pass_tsc_ticks'] >= values['executed_service_tsc_ticks'],
                    'whole combining passes undercount their executed callback service')
        requests.append(count)
        service.append(sums)
        before.append(early)
        tsc.append(ticks)
        before_tsc.append(early_ticks)
        executors.append(values)
    operations = sum(completed)
    total_requests = sum(map(sum, requests))
    require(total_requests == (operations if bridge else 0),
            'bridge request counts do not equal operations')
    require(global_counts['accepted_calls'] ==
            (total_requests if instrumented else 0) ==
            global_counts['completed_calls'] and
            global_counts['active_calls'] == 0 and
            global_counts['rejected_reentrant'] == 0,
            'bridge profile completion/in-flight/reentry mismatch')
    require(global_counts['peak_inflight_calls'] <=
            (total_requests if instrumented else 0) and
            (not instrumented or operations == 0 or global_counts['peak_inflight_calls'] > 0),
            'invalid peak bridge in-flight count')
    if instrumented:
        require(sum(v['executed_callbacks'] for v in executors) == total_requests and
                sum(v['executed_service_ns'] for v in executors) == sum(map(sum, service)) and
                sum(v['executed_service_tsc_ticks'] for v in executors) == sum(map(sum, tsc)),
                'summed requester service/count differs from physical executor totals')
    return {'requests': requests, 'service_ns': service, 'before_deadline_ns': before,
            'service_tsc_ticks': tsc, 'before_deadline_tsc_ticks': before_tsc,
            'executors': executors, 'global': global_counts, 'instrumented': instrumented}


def validate(record, manifest, baseline_flags):
    cfg = manifest['config']
    result = record.get('result')
    require(record.get('success') is True and record.get('returncode') == 0 and
            not record.get('timed_out') and not record.get('launch_error'), 'process failed/timeout')
    require(isinstance(result, dict) and result.get('schema') == 1 and
            result.get('status') == 'ok', 'invalid/failed harness JSON')
    require(isinstance(record.get('stdout'), str) and json.loads(record['stdout']) == result,
            'saved parsed result differs from raw stdout')
    for name in ('stdout', 'stderr'):
        text = record.get(name)
        encoded = record.get(name + '_base64')
        require(isinstance(text, str) and isinstance(encoded, str) and
                base64.b64decode(encoded, validate=True).decode('utf-8', errors='replace') == text,
                name + ': exact captured bytes differ from display text')
    variant = record['variant']
    require(result.get('variant') == variant, 'harness variant does not match executable label')
    if variant in PROFILES and variant != 'profile':
        label = result.get('bridge_profile_semantics')
        require(isinstance(label, str) and 'TSC' in label and 'CPU' in label,
                'bridge profile must label TSC as elapsed, not CPU')
    require(result.get('mode') == cfg['mode'] and result.get('seed') == cfg['seed'],
            'mode/seed mismatch')
    for actual, expected in (('cpu_list', cfg['cpus']), ('finders', cfg['finders']),
                             ('inserters', cfg['inserters']), ('preload', cfg['preload']),
                             ('max_inserts', cfg['max_inserts']),
                             ('memory_limit_gib', cfg['memory_limit_gib']),
                             ('warmup_seconds', cfg['warmup']),
                             ('duration_requested_seconds', cfg['seconds']),
                             ('wait_proxy_threshold_ns', cfg['wait_proxy_ns'])):
        require(result.get(actual) == expected, actual + ' differs from manifest')
    require(result.get('fixed_requested') == {'find': cfg['reads'], 'insert': cfg['inserts']},
            'fixed requested counts differ')
    require(result.get('setup_cpu') == cfg.get('init_cpu', cfg['cpus'][0]) and
            result.get('warmup_status') == ('skipped' if cfg['warmup'] == 0 else 'verified_and_destroyed'),
            'setup CPU/warmup differs')
    if 'init_node' in cfg:
        require(result.get('init_node') == cfg['init_node'], 'initialization node differs')
    requested = cfg.get('requested_memory_policy')
    if requested is not None:
        placement = result.get('numa_memory')
        require(isinstance(placement, dict) and placement.get('requested') == requested and
                placement.get('effective') == requested and placement.get('explicit') is True and
                placement.get('nodes') == cfg.get('memory_nodes', []) and
                placement.get('effective_nodes') == cfg.get('memory_nodes', []),
                'memory policy or effective nodemask not enforced as requested')
        for phase in ('after_setup', 'after_work'):
            pages = placement.get(phase)
            require(isinstance(pages, dict) and
                    pages.get('scope') == 'whole_process_including_libraries_stacks_and_db' and
                    pages.get('unit') == 'kernel_pages' and
                    isinstance(pages.get('nodes'), dict) and
                    sum(integer(n, 'resident pages') for n in pages['nodes'].values()) ==
                    integer(pages.get('resident_pages'), 'resident_pages') and
                    integer(pages.get('mapped_regions'), 'mapped_regions') > 0,
                    phase + ': invalid process NUMA residency')
    flags = {key: result.get(key) for key in FLAGS}
    build = manifest['artifacts']['binaries'][variant]['build_manifest']
    require(flags['upstream_sha'] == build.get('pinned_sha') and
            build.get('hashes', {}).get('harness_sha256') ==
            manifest['artifacts']['native_harness_sha256'],
            'harness output/workload differs from authoritative build')
    require(all(value is not None for key, value in flags.items() if key != 'insert_key_formula'),
            'missing workload flags')
    require(flags['insert_key_formula'] is None or
            (isinstance(flags['insert_key_formula'], str) and flags['insert_key_formula']),
            'invalid insert key formula')
    require(flags['environment'] == 'UPS_IN_MEMORY' and flags['record_bytes'] == 8 and
            flags['key_bytes'] == 8 and flags['transactions'] is False and
            flags['recovery'] is False and flags['duplicates'] is False and
            flags['compression'] is False and flags['direct_access'] is False,
            'unsupported workload or data representation')
    require(baseline_flags is None or flags == baseline_flags, 'workload flags differ across trials')
    totals = result.get('totals')
    verification = result.get('verification')
    shutdown = result.get('shutdown')
    workers = result.get('workers')
    require(isinstance(totals, dict) and isinstance(verification, dict) and
            isinstance(shutdown, dict) and isinstance(workers, list) and
            len(workers) == cfg['workers'], 'missing totals/verification/shutdown/workers')
    require(all(shutdown.get(k) == 0 for k in ('db_status', 'env_status')),
            'shutdown failure')
    for field in ('missing_reads', 'duplicate_inserts', 'other_errors', 'bad_value', 'bad_size'):
        require(totals.get(field) == 0, 'total ' + field + ' nonzero')
    for field in ('integrity_status', 'cursor_status', 'count_status', 'bad_entries'):
        require(verification.get(field) == 0, 'verification ' + field + ' nonzero')
    require(verification.get('expected_count') == verification.get('seen') ==
            verification.get('db_count'), 'final enumeration/count mismatch')
    attempted = pair(totals.get('attempted'), 'totals.attempted')
    completed = pair(totals.get('completed'), 'totals.completed')
    on_time = pair(totals.get('completed_before_deadline'), 'totals.on_time')
    late = pair(totals.get('completed_during_drain'), 'totals.late')
    require(all(a == b for a, b in zip(attempted, completed)) and
            all(c == t + l for c, t, l in zip(completed, on_time, late)),
            'failed, omitted, or misclassified operations')
    expected_insert = completed[1] if cfg['mode'] == 'duration' else cfg['inserts']
    require(verification['expected_count'] == cfg['preload'] + expected_insert,
            'final database size does not match inserts')
    elapsed = integer(result.get('elapsed_ns'), 'elapsed_ns')
    drain = integer(result.get('drain_ns'), 'drain_ns')
    deadline = integer(result.get('deadline_offset_ns'), 'deadline_offset_ns')
    require(elapsed > 0, 'nonpositive elapsed')
    if cfg['mode'] == 'fixed':
        require(completed == [cfg['reads'], cfg['inserts']] and on_time == completed and
                late == [0, 0] and deadline == drain == 0, 'fixed work incomplete')
    else:
        require(abs(deadline - cfg['seconds'] * 1e9) < 1000 and
                drain == max(0, elapsed - deadline), 'deadline/drain inconsistent')
    per_worker = []
    require(all(isinstance(w, dict) and isinstance(w.get('latency_ns'), dict) and
                isinstance(w.get('native_mutex_profile'), dict) for w in workers),
            'malformed worker histograms')
    for index, w in enumerate(workers):
        require(w.get('id') == index and w.get('cpu_requested') == cfg['cpus'][index % len(cfg['cpus'])]
                and w.get('cpu_start') == w.get('cpu_requested') and
                w.get('cpu_end') == w.get('cpu_requested') and
                w.get('finder') is (index < cfg['finders']) and
                w.get('inserter') is (cfg['inserters'] == 0 or index >= cfg['finders']),
                'worker role/effective affinity mismatch')
        require(all(w.get(field) == 0 for field in
                    ('missing_reads', 'duplicate_inserts', 'other_errors', 'bad_value',
                     'bad_size', 'first_db_status', 'affinity_error',
                     'missing_profile_samples', 'bridge_status')),
                'worker errors/status nonzero')
        wc = pair(w.get('completed'), 'worker.completed')
        wa = pair(w.get('attempted'), 'worker.attempted')
        wt = pair(w.get('completed_before_deadline'), 'worker.on_time')
        wl = pair(w.get('completed_during_drain'), 'worker.late')
        require(wc == wa and all(c == t + l for c, t, l in zip(wc, wt, wl)),
                'worker completion mismatch')
        for op_index, op in enumerate(OP_NAMES):
            require(isinstance(w['native_mutex_profile'].get(op), dict),
                    'malformed native mutex profile')
            _, n, _, _ = histogram(w.get('latency_ns', {}).get(op), 'latency.' + op)
            require(n == wa[op_index], 'latency sample loss for ' + op)
            profile = w.get('native_mutex_profile', {}).get(op, {})
            for field in ('wait_ns', 'hold_pre_release_ns'):
                _, pn, _, _ = histogram(profile.get(field), field + '.' + op)
                require(pn == (wa[op_index] if variant == 'profile' else 0),
                        'profile sample count mismatch')
                if variant != 'profile':
                    require(profile[field].get('sum_ns') == 0,
                            'disabled native mutex profile must not report measured time')
            proxy = integer(profile.get('wait_over_threshold_proxy'), 'wait threshold proxy')
            require(proxy <= wa[op_index] if variant == 'profile' else proxy == 0,
                    'native mutex proxy count inconsistent')
        per_worker.append((wc, wt))
    for idx in range(2):
        require(sum(w[0][idx] for w in per_worker) == completed[idx] and
                sum(w[1][idx] for w in per_worker) == on_time[idx],
                'worker sum does not match totals')
    bridge_data = bridge_metrics(result, workers, variant, completed, on_time, cfg['mode'])
    process_cpu = integer(record.get('process_cpu_time_ns'), 'process_cpu_time_ns')
    require(process_cpu >= 0, 'invalid process CPU time')
    interval = elapsed / 1e9 if cfg['mode'] == 'fixed' else deadline / 1e9
    counted = completed if cfg['mode'] == 'fixed' else on_time
    metrics = {'elapsed_s': elapsed / 1e9, 'drain_s': drain / 1e9,
               'process_cpu_s': process_cpu / 1e9,
               'worker_cpu_s': sum(integer(w.get('cpu_time_ns'), 'worker.cpu_time_ns')
                                   for w in workers) / 1e9,
               'total_ops_s': sum(counted) / interval,
               'find_ops_s': counted[0] / interval, 'insert_ops_s': counted[1] / interval,
               'writer_max_no_progress_s': max((integer(
                   w.get('longest_writer_no_progress_ns'), 'writer gap') for w in workers
                   if w['inserter']), default=0) / 1e9}
    # Harness process usage is timed work only. Runner child CPU spans setup, warmup,
    # work and teardown; do not merge these differently scoped measurements.
    if 'process_cpu_time_ns' in result:
        metrics['timed_process_cpu_s'] = integer(result['process_cpu_time_ns'],
                                                 'timed process CPU time') / 1e9
    if 'peak_rss_process_lifetime_kib' in result:
        metrics['peak_rss_process_lifetime_kib'] = integer(
            result['peak_rss_process_lifetime_kib'], 'lifetime peak RSS')
    if 'phase_rusage' in result:
        require(isinstance(result['phase_rusage'], dict), 'malformed timed resource usage')
        for name in ('involuntary_context_switches', 'voluntary_context_switches',
                     'minor_faults', 'major_faults', 'block_input', 'block_output'):
            metrics['timed_' + name] = integer(result['phase_rusage'].get(name),
                                                'phase_rusage.' + name)
    if variant == 'profile':
        for idx, op in enumerate(OP_NAMES):
            proxy = sum(w['native_mutex_profile'][op]['wait_over_threshold_proxy']
                        for w in workers)
            metrics[op + '_wait_over_threshold_proxy_count'] = proxy
            metrics[op + '_wait_over_threshold_proxy_fraction'] = (
                proxy / attempted[idx] if attempted[idx] else None)
    histograms = {}
    for operation in OP_NAMES:
        for kind, field in (('latency', 'latency_ns'), ('wait', 'wait_ns'),
                            ('hold_pre_release', 'hold_pre_release_ns')):
            if kind != 'latency' and variant != 'profile' and not any(
                    w['native_mutex_profile'][operation][field]['samples'] for w in workers):
                continue
            group = metrics_for_histograms(workers, field, operation)
            # All histogram means require sum_ns from every worker; never infer from bins.
            histograms[f'{operation}_{kind}'] = group
            metrics[f'{operation}_{kind}_samples'] = group['samples']
            metrics[f'{operation}_{kind}_mean_ns'] = group['mean_ns']
            for q, upper in group['quantile_upper_bound_ns'].items():
                metrics[f'{operation}_{kind}_{q}_upper_ns'] = upper
    progress = result.get('progress')
    require(isinstance(progress, list) and progress, 'missing progress observations')
    previous = -1
    prior_find = prior_insert = 0
    for point in progress:
        require(isinstance(point, dict) and isinstance(point.get('worker_find'), list) and
                isinstance(point.get('worker_insert'), list) and
                isinstance(point.get('writer_since_last_ns'), list),
                'malformed progress sample')
        offset = integer(point.get('offset_ns'), 'progress.offset_ns')
        require((offset >= previous or point is progress[-1]) and
                len(point['worker_find']) == cfg['workers'] and
                len(point['worker_insert']) == cfg['workers'] and
                len(point['writer_since_last_ns']) == cfg['workers'],
                'invalid progress timeline')
        finds = integer(point.get('find'), 'progress.find')
        inserts = integer(point.get('insert'), 'progress.insert')
        require(prior_find <= finds <= completed[0] and
                prior_insert <= inserts <= completed[1] and
                sum(integer(n, 'progress.worker_find') for n in point['worker_find']) == finds and
                sum(integer(n, 'progress.worker_insert') for n in point['worker_insert']) == inserts,
                'progress counts inconsistent')
        prior_find, prior_insert = finds, inserts
        previous = offset
    require(progress[-1].get('find') == completed[0] and
            progress[-1].get('insert') == completed[1], 'terminal progress mismatch')
    # Service is elapsed time in a measured callback, attributed to the originating
    # requester; executed CPU time belongs to the physical worker and is separate.
    service = bridge_data
    if service['instrumented']:
        full = [sum(values) for values in service['service_ns']]
        observed = [sum(values) for values in (service['before_deadline_ns']
                    if cfg['mode'] == 'duration' else service['service_ns'])]
        full_ticks = [sum(values) for values in service['service_tsc_ticks']]
        observed_ticks = [sum(values) for values in (service['before_deadline_tsc_ticks']
                          if cfg['mode'] == 'duration' else service['service_tsc_ticks'])]
        metrics['requester_service_ns_all_including_drain'] = sum(full)
        metrics['requester_service_tsc_ticks_all_including_drain'] = sum(full_ticks)
        metrics['requester_service_ns_observed'] = sum(observed)
        metrics['requester_service_tsc_ticks_observed'] = sum(observed_ticks)
        metrics['physical_executor_callback_elapsed_ns'] = sum(
            e['executed_service_ns'] for e in service['executors'])
        metrics['physical_executor_callback_elapsed_tsc_ticks'] = sum(
            e['executed_service_tsc_ticks'] for e in service['executors'])
        metrics['physical_executor_callback_count'] = sum(
            e['executed_callbacks'] for e in service['executors'])
        if all(e['has_combiner_pass_ticks'] for e in (w['bridge_profile']['executor']
                                                      for w in workers)):
            metrics['physical_executor_whole_pass_elapsed_tsc_ticks'] = sum(
                e['combiner_pass_tsc_ticks'] for e in service['executors'])
        for op_index, op in enumerate(OP_NAMES):
            hist = metrics_for_histograms_bridge(workers, op)
            histograms[op + '_requester_service'] = hist
            metrics[op + '_requester_service_samples'] = hist['samples']
            metrics[op + '_requester_service_mean_ns'] = hist['mean_ns']
            for q, upper in hist['quantile_upper_bound_ns'].items():
                metrics[op + '_requester_service_' + q + '_upper_ns'] = upper
        for role, indexes in role_groups(cfg):
            amounts = [observed[i] for i in indexes]
            metrics['requester_' + role + '_service_ns_observed'] = sum(amounts)
            total_service = sum(observed)
            metrics['requester_' + role + '_service_share'] = (
                sum(amounts) / total_service if total_service else None)
            if cfg['mode'] == 'duration':
                # Workers share one response-deadline observation interval.
                # A fixed-work quota is NOT a fairness experiment.
                metrics['requester_' + role + '_service_jain'] = jain(amounts)
    if cfg['mode'] == 'duration':
        counts = [sum(w[1]) for w in per_worker]
        for role, indexes in role_groups(cfg):
            amounts = [counts[i] for i in indexes]
            metrics['requester_' + role + '_completed_before_deadline_total'] = sum(amounts)
            metrics['requester_' + role + '_completed_count_jain'] = jain(amounts)
    return {'variant': variant, 'block': record['block'], 'metrics': metrics,
            'histograms': histograms, 'completed': completed, 'on_time': on_time,
            'during_drain': late, 'progress': progress, 'flags': flags,
            'process_resource_scope': result.get('process_resource_scope'),
            'bridge_profile': service, 'per_worker_completed': [w[0] for w in per_worker],
            'per_worker_before_deadline': [w[1] for w in per_worker]}


def ci(values, rng, draws):
    if len(values) < 10:
        return None
    boots = [statistics.mean(rng.choices(values, k=len(values))) for _ in range(draws)]
    boots.sort()
    return [boots[int(0.025 * (draws - 1))], boots[int(0.975 * (draws - 1))]]


def aggregate(trials, rng, draws):
    names = sorted({key for trial in trials for key in trial['metrics']})
    result = {}
    for name in names:
        values = [trial['metrics'].get(name) for trial in trials]
        present = [v for v in values if v is not None]
        if not present:
            continue
        result[name] = {'mean': statistics.mean(present), 'n': len(present),
                        'bootstrap95': ci(present, rng, draws),
                        'exploratory': len(present) < 10,
                        'units': 'ns upper bound (histogram)' if name.endswith('_upper_ns')
                                 else ('ns exact (sum_ns)' if name.endswith('_mean_ns') else None)}
    return result


def comparisons(by_variant, mode, rng, draws):
    primary = [(a, b, 'primary_stage') for a, b in zip(PRIMARY[:5], PRIMARY[1:5])]
    primary.append(('native', 'fc_pq', 'overall_primary'))
    primary.extend((reference, baseline, 'conventional_baseline')
                   for baseline in ('uscl', 'cfl_local', 'spinlock', 'mcs', 'ticket', 'clh')
                   for reference in ('native', 'fc_pq'))
    instrumented = [(a, b, 'instrumented_mode') for a, b in
                    zip(PROFILES[:4], PROFILES[1:4])]
    overhead_pairs = [(base, profile, 'instrumentation_overhead')
                      for profile, base in PROFILE_BASE.items()]
    output = []
    for left, right, category in primary + instrumented + overhead_pairs:
        first = {t['block']: t for t in by_variant.get(left, [])}
        second = {t['block']: t for t in by_variant.get(right, [])}
        blocks = sorted(first.keys() & second.keys())
        if not blocks:
            continue
        names = ('elapsed_s',) if mode == 'fixed' else ('find_ops_s', 'insert_ops_s', 'total_ops_s')
        for name in names:
            positive = [b for b in blocks if first[b]['metrics'][name] > 0]
            skipped = sorted(set(blocks) - set(positive))
            if mode == 'fixed':
                ratios = [first[b]['metrics'][name] / second[b]['metrics'][name] for b in positive]
                kind = 'fixed_work_speedup'
            else:
                ratios = [second[b]['metrics'][name] / first[b]['metrics'][name] for b in positive]
                kind = ('on_time_mixed_total_ops_s_ratio_mixes_may_differ' if name == 'total_ops_s'
                        else 'on_time_' + name + '_ratio')
            overhead = ([(second[b]['metrics']['elapsed_s'] /
                          first[b]['metrics']['elapsed_s'] - 1) * 100 for b in positive]
                        if mode == 'fixed' else None)
            output.append({'from': left, 'to': right, 'category': category,
                           'instrumentation_gate': category == 'instrumentation_overhead',
                           'blocks': blocks, 'used_blocks': positive,
                           'zero_denominator_blocks': skipped, 'n': len(positive),
                           'metric': name, 'ratio_kind': kind,
                           'ratio': {'mean': statistics.mean(ratios) if ratios else None,
                                     'bootstrap95': ci(ratios, rng, draws), 'n': len(ratios)},
                           'elapsed_overhead_percent': None if overhead is None else {
                               'mean': statistics.mean(overhead), 'bootstrap95': ci(overhead, rng, draws)},
                           'exploratory': len(ratios) < 10,
                           'note': ('instrumentation overhead: NOT a primary algorithm comparison'
                                    if category == 'instrumentation_overhead' else
                                    'mixed total may change operation mix; NOT a fixed-work speedup'
                                    if name == 'total_ops_s' and mode == 'duration' else None)})
    return output


def plot_bar(plt, groups, field, ylabel, path):
    data = [(variant, groups[variant]['metrics'][field]) for variant in groups
            if field in groups[variant]['metrics'] and groups[variant]['metrics'][field]['n'] > 0]
    if not data:
        return
    fig, ax = plt.subplots(figsize=(max(7, len(data)*1.4), 4))
    for idx, (variant, stat) in enumerate(data):
        ax.bar(idx, stat['mean'], color='#a87048' if variant in PROFILES else '#4778a8')
        if stat['bootstrap95'] is not None:
            low, high = stat['bootstrap95']
            ax.errorbar(idx, stat['mean'], yerr=[[max(0, stat['mean'] - low)],
                                                 [max(0, high - stat['mean'])]], fmt='none', color='black')
    ax.set_xticks(range(len(data)), [name.replace('_', '\n') for name, _ in data])
    ax.set_ylabel(ylabel)
    ax.set_title(field + ' (trial-level mean; 95% bootstrap CI if n>=10)')
    fig.tight_layout()
    fig.savefig(path, dpi=150)
    plt.close(fig)


def plot_pairs(plt, paired, category, metric, key, ylabel, path):
    data = [(c, c[key]) for c in paired if c['category'] == category and
            c['metric'] == metric and c[key] is not None and c[key]['mean'] is not None]
    if not data:
        return
    fig, ax = plt.subplots(figsize=(max(7, len(data)*1.6), 4))
    for i, (entry, stat) in enumerate(data):
        center = stat['mean']
        ax.bar(i, center, color='#a87048' if category == 'instrumentation_overhead' else '#4778a8')
        if stat['bootstrap95']:
            low, high = stat['bootstrap95']
            ax.errorbar(i, center, yerr=[[max(0, center - low)],
                                          [max(0, high - center)]], fmt='none', color='black')
    ax.axhline(0 if key == 'elapsed_overhead_percent' else 1, color='black', linewidth=1)
    ax.set_xticks(range(len(data)), [c['from'].replace('_', ' ') + '\n→ ' +
                                     c['to'].replace('_', ' ') for c, _ in data])
    ax.set_ylabel(ylabel)
    ax.set_title(category.replace('_', ' ') + ': trial-paired means (95% CI if n>=10)')
    fig.tight_layout()
    fig.savefig(path, dpi=150)
    plt.close(fig)


def plots(plt, output, by_variant, groups, paired, mode):
    primary = {v: groups[v] for v in PRIMARY if v in groups}
    profiles = {v: groups[v] for v in PROFILES if v in groups}
    if mode == 'fixed':
        plot_bar(plt, primary, 'elapsed_s', 'elapsed s; same completed work',
                 output / 'fixed_elapsed.png')
        plot_bar(plt, profiles, 'elapsed_s', 'profile elapsed s; same completed work',
                 output / 'profile_fixed_elapsed.png')
        for category, filename in (('primary_stage', 'fixed_speedup.png'),
                                   ('overall_primary', 'fixed_native_to_fc_pq_speedup.png'),
                                   ('conventional_baseline', 'fixed_conventional_baselines.png'),
                                   ('instrumented_mode', 'profile_fixed_speedup.png')):
            plot_pairs(plt, paired, category, 'elapsed_s', 'ratio', 'fixed-work speedup',
                       output / filename)
        plot_pairs(plt, paired, 'instrumentation_overhead', 'elapsed_s',
                   'elapsed_overhead_percent', 'instrumentation elapsed overhead %',
                   output / 'profile_instrumentation_overhead.png')
    else:
        for category, prefix in (('primary_stage', 'duration_primary'),
                                 ('overall_primary', 'duration_overall'),
                                 ('conventional_baseline', 'duration_conventional_baselines'),
                                 ('instrumented_mode', 'duration_profile'),
                                 ('instrumentation_overhead', 'duration_instrumentation')):
            for op in ('find', 'insert', 'total'):
                plot_pairs(plt, paired, category, op + '_ops_s', 'ratio',
                           ('matching profile / primary ' if category == 'instrumentation_overhead' else '') +
                           op + ' before-deadline ops/s ratio' +
                           ('; MIX MAY DIFFER' if op == 'total' else ''),
                           output / (prefix + '_' + op + '_ratio.png'))
    for op in OP_NAMES:
        for category, selected in (('', primary), ('profile_', profiles)):
            plot_bar(plt, selected, op + '_ops_s', op + ' completed ops/s' +
                     (' before deadline' if mode == 'duration' else ''),
                     output / (category + op + '_throughput.png'))
            for kind in ('latency', 'wait', 'hold_pre_release', 'requester_service'):
                plot_bar(plt, selected, op + '_' + kind + '_mean_ns',
                         op + ' measured ' + kind + ' mean ns',
                         output / (category + op + '_' + kind + '_mean.png'))
                for quantile in ('p95', 'p99', 'p99_9'):
                    plot_bar(plt, selected, op + '_' + kind + '_' + quantile + '_upper_ns',
                             op + ' ' + kind + ' ' + quantile + ' ns histogram upper bound',
                             output / (category + op + '_' + kind + '_' + quantile + '_upper.png'))
    for category, selected in (('', primary), ('profile_', profiles)):
        plot_bar(plt, selected, 'writer_max_no_progress_s',
                 'longest writer no-progress interval s', output / (category + 'writer_no_progress.png'))
        if mode == 'duration':
            for role in ('all', 'finder', 'inserter', 'mixed'):
                for kind, label in (('service_jain', 'measured requester service ns'),
                                    ('completed_count_jain', 'completed operation count')):
                    plot_bar(plt, selected, 'requester_' + role + '_' + kind,
                             role + ' Jain index: ' + label + '\n(responses completed by deadline)',
                             output / (category + 'requester_' + role + '_' + kind + '.png'))
                plot_bar(plt, selected, 'requester_' + role + '_service_share',
                         role + ' share of measured service of responses by deadline',
                         output / (category + 'requester_' + role + '_service_share.png'))
        for field, ylabel in (
                ('physical_executor_callback_elapsed_ns', 'physical executor callback elapsed ns; includes drain'),
                ('physical_executor_whole_pass_elapsed_tsc_ticks', 'whole combining-pass elapsed TSC ticks, NOT CPU cycles')):
            plot_bar(plt, selected, field, ylabel, output / (category + field + '.png'))
    if mode == 'duration':
        for category, selected in (('', PRIMARY), ('profile_', PROFILES)):
            fig, ax = plt.subplots(figsize=(9, 5))
            plotted = False
            for name in selected:
                trials = by_variant.get(name, [])
                for trial in trials:
                    points = trial['progress']
                    ax.plot([p['offset_ns']/1e9 for p in points],
                            [p['insert'] for p in points], alpha=0.32,
                            label=name if trial is trials[0] else None)
                    plotted = True
            if plotted:
                ax.axvline(next(iter(groups.values()))['requested_seconds'], color='black',
                           linestyle='--', label='deadline (drain to right)')
                ax.set_xlabel('seconds since work start')
                ax.set_ylabel('completed inserts; includes drain after deadline')
                ax.legend()
                fig.tight_layout()
                fig.savefig(output / (category + 'writer_progress.png'), dpi=150)
            plt.close(fig)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--input-dir', type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument('--output-dir', type=Path, help='default INPUT/analysis')
    parser.add_argument('--no-plots', action='store_true',
                        help='write validated summary/CSV only; defer plotting until after timed runs')
    parser.add_argument('--bootstrap-draws', type=int, default=4000)
    parser.add_argument('--bootstrap-seed', type=int, default=20260923)
    args = parser.parse_args()
    if args.bootstrap_draws < 100:
        parser.error('bootstrap-draws must be >= 100')
    input_dir = args.input_dir.resolve()
    output = (args.output_dir or (input_dir / 'analysis')).resolve()
    manifest = json.loads((input_dir / 'manifest.json').read_text(), parse_constant=reject_nonfinite)
    require(manifest.get('schema') == 1, 'unsupported runner manifest schema')
    cfg = manifest['config']
    require(len(set(cfg['variants'])) == len(cfg['variants']) and
            all(v in VARIANTS for v in cfg['variants']), 'invalid variant matrix')
    require(len(manifest['orders']) == cfg['repetitions'] and
            len(manifest['trial_files']) == cfg['repetitions'] * len(cfg['variants']),
            'manifest trial matrix incomplete')
    planned = [f'block-{b:04d}.{v}.json'
               for b, order in enumerate(manifest['orders']) for v in order]
    require(all(sorted(order) == sorted(cfg['variants']) for order in manifest['orders']) and
            manifest['trial_files'] == planned, 'trial orders/files mismatch')
    archive = manifest['artifacts']['integration_sources']
    require(digest(input_dir / archive['file']) == archive['sha256'] and
            archive['files'].get('integration/upscaledb/core/bridge.h') ==
            manifest['artifacts']['bridge_header_sha256'] and
            archive['files'].get('integration/upscaledb/core/native-lock-timing.patch') ==
            manifest['artifacts']['native_profile_patch_sha256'],
            'missing, corrupted, or inconsistent archived integration sources')
    for variant in cfg['variants']:
        artifact = manifest['artifacts']['binaries'][variant]
        build = artifact['build_manifest']
        require(build.get('variant') == variant and
                build.get('hashes', {}).get('binary_sha256') == artifact['sha256'] and
                artifact['build_manifest_sha256'] is not None,
                variant + ': executable/build provenance mismatch')
    flags = None
    by_variant = {v: [] for v in cfg['variants']}
    failures = []
    for file in manifest['trial_files']:
        try:
            rec = json.loads((input_dir / file).read_text(), parse_constant=reject_nonfinite)
            block = rec['block']
            variant = rec['variant']
            require(rec.get('schema') == 1 and file == f'block-{block:04d}.{variant}.json' and
                    0 <= block < cfg['repetitions'] and variant in cfg['variants'] and
                    rec.get('config') == cfg and rec.get('machine') == manifest['machine'] and
                    rec.get('artifacts') == manifest['artifacts'] and
                    rec.get('order_seed') == cfg['order_seed'] and
                    rec.get('order') == manifest['orders'][block] and
                    rec.get('position') == manifest['orders'][block].index(variant),
                    'record/manifest configuration or order mismatch')
            binary = manifest['artifacts']['binaries'][variant]
            require(rec.get('command', [None])[0] == binary['path'], 'binary command mismatch')
            if 'commands' in manifest:
                require(rec.get('command') == manifest['commands'].get(file),
                        'trial command differs from pre-recorded manifest command')
            require(rec.get('binary_sha256_before') == binary['sha256'] and
                    rec.get('binary_sha256_after') == binary['sha256'] and
                    rec.get('artifact_error') is None,
                    'executable SHA-256 changed or was unavailable during trial')
            summary = validate(rec, manifest, flags)
            flags = summary['flags']
            by_variant[variant].append(summary)
        except (BadTrial, ValueError, KeyError, IndexError, TypeError, OSError, binascii.Error) as exc:
            failures.append({'file': file, 'reason': str(exc)})
    rng = random.Random(args.bootstrap_seed)
    groups = {variant: {'kind': 'profile' if variant in PROFILES else 'primary',
                        'n_success': len(trials), 'n_expected': cfg['repetitions'],
                        'n_failed_or_missing': cfg['repetitions'] - len(trials),
                        'exploratory': len(trials) < 10, 'requested_seconds': cfg['seconds'],
                        'metrics': aggregate(trials, rng, args.bootstrap_draws)}
              for variant, trials in by_variant.items()}
    paired = comparisons(by_variant, cfg['mode'], rng, args.bootstrap_draws)
    matplotlib = plt = None
    if not args.no_plots:
        try:
            import matplotlib
            matplotlib.use('Agg')
            import matplotlib.pyplot as plt
        except ImportError as exc:
            parser.error('plots need matplotlib >= 3.8: ' + str(exc))
        version = tuple(int(part) for part in matplotlib.__version__.split('.')[:2])
        if version < (3, 8):
            parser.error('plots need matplotlib >= 3.8')
    output.mkdir(parents=True, exist_ok=True)
    info = {'schema': 1, 'config': cfg, 'source_manifest': str(input_dir / 'manifest.json'),
            'classification': 'smoke_not_primary' if cfg['smoke'] else
                              ('exploratory' if cfg['repetitions'] < 10 else
                               'candidate_primary_and_separate_profiles'),
            'raw_failures': failures, 'failure_count': len(failures),
            'workload_flags': flags, 'groups': groups, 'paired_comparisons': paired,
            'trials': {name: trials for name, trials in by_variant.items()},
            'bootstrap': {'method': 'resample independent trial summaries with replacement',
                          'draws': args.bootstrap_draws, 'seed': args.bootstrap_seed,
                          'minimum_n_for_ci': 10},
            'analysis_environment': {'python': sys.version,
                                     'matplotlib': matplotlib.__version__ if matplotlib else None,
                                     'backend': matplotlib.get_backend() if matplotlib else None},
            'notes': ['Manifest prewrites the full randomized trial matrix; every missing/invalid/nonzero/timed-out trial is named in raw_failures and failures.csv, never silently discarded.',
                      'Only validated successful trials contribute estimates; failed trials remain raw and are not replaced or pooled.',
                      'All uncertainty resamples whole fresh-process trial blocks (paired for ratios), not individual callbacks.',
                      'Disabled counters are UNMEASURED, not zero measurements: absent from aggregate metrics and profile plots.',
                      'NUMA resident-page snapshots count the whole process including libraries and stacks, not isolated database pages; effective policy is checked separately.',
                      'Requester callback service is elapsed nanoseconds and elapsed TSC ticks attributed to request origin; physical executor callbacks and whole combining-pass TSC are separate. TSC ticks are NOT CPU cycles or CPU time.',
                      'Requester full service includes drain. Duration service fairness sums whole callback service only for requests whose RESPONSE finished by deadline, across ALL active workers with the same response-deadline observation interval; it is not the exact integral of callback time inside the window and does not partially credit late responses.',
                      'Completed-count Jain indices are separate from service Jain indices; fixed-work quotas do not establish fairness.',
                      'Duration ratios report operation-specific completed throughput; mixed total may change operation mix and is NOT fixed-work speedup.',
                      'Profile-to-profile compares instrumented modes only; primary-to-matching-profile measures instrumentation overhead, not an algorithm ranking.',
                      'Histogram quantiles are upper bounds; bin63 overflow has no finite bound; insufficient sample support omits quantiles.',
                      'Old sequential insert-key archives must NOT be pooled with new high-bit permuted-key runs.',
                      'Small n (<10) exploratory; no robust bootstrap CI reported.']}
    (output / 'summary.json').write_text(json.dumps(info, indent=2, allow_nan=False) + '\n')
    with (output / 'summary.csv').open('w', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=('variant', 'metric', 'mean', 'n', 'ci_low', 'ci_high', 'exploratory'))
        writer.writeheader()
        for variant, group in groups.items():
            for metric, stat in group['metrics'].items():
                interval = stat['bootstrap95'] or [None, None]
                writer.writerow({'variant': variant, 'metric': metric, 'mean': stat['mean'],
                                 'n': stat['n'], 'ci_low': interval[0], 'ci_high': interval[1],
                                 'exploratory': stat['exploratory']})
    with (output / 'failures.csv').open('w', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=('file', 'reason'))
        writer.writeheader()
        writer.writerows(failures)
    with (output / 'trials.csv').open('w', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=('variant', 'block', 'kind', 'metric', 'value'))
        writer.writeheader()
        for variant, trials in by_variant.items():
            for trial in trials:
                for name, value in trial['metrics'].items():
                    if value is not None:
                        writer.writerow({'variant': variant, 'block': trial['block'],
                                         'kind': groups[variant]['kind'], 'metric': name, 'value': value})
    with (output / 'worker_service.csv').open('w', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=('variant', 'block', 'worker', 'role',
                                                    'requested_calls', 'completed_before_deadline',
                                                    'requester_service_ns_including_drain',
                                                    'requester_service_before_deadline_ns',
                                                    'requester_service_tsc_ticks_including_drain',
                                                    'executor_callbacks',
                                                    'executor_callback_elapsed_ns',
                                                    'executor_whole_pass_elapsed_tsc_ticks'))
        writer.writeheader()
        for variant, trials in by_variant.items():
            for trial in trials:
                profile = trial['bridge_profile']
                if not profile['instrumented']:
                    continue
                for index in range(cfg['workers']):
                    executor = profile['executors'][index]
                    writer.writerow({'variant': variant, 'block': trial['block'], 'worker': index,
                                     'role': ('mixed' if cfg['inserters'] == 0 else
                                              'finder' if index < cfg['finders'] else 'inserter'),
                                     'requested_calls': sum(profile['requests'][index]),
                                     'completed_before_deadline':
                                     sum(trial['per_worker_before_deadline'][index]),
                                     'requester_service_ns_including_drain': sum(profile['service_ns'][index]),
                                     'requester_service_before_deadline_ns': sum(profile['before_deadline_ns'][index]),
                                     'requester_service_tsc_ticks_including_drain': sum(profile['service_tsc_ticks'][index]),
                                     'executor_callbacks': executor['executed_callbacks'],
                                     'executor_callback_elapsed_ns': executor['executed_service_ns'],
                                     'executor_whole_pass_elapsed_tsc_ticks':
                                     executor['combiner_pass_tsc_ticks'] if
                                     profile['instrumented'] and variant in ('fc_profile', 'fc_pq_profile') else ''})
    with (output / 'paired.csv').open('w', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=('from', 'to', 'category', 'metric', 'kind', 'n',
                                                     'mean', 'ci_low', 'ci_high', 'exploratory',
                                                     'paired_blocks', 'zero_denominator_blocks'))
        writer.writeheader()
        for pair_info in paired:
            entries = [(pair_info['ratio_kind'], pair_info['ratio'])]
            if pair_info['category'] == 'instrumentation_overhead' and pair_info['elapsed_overhead_percent']:
                entries.append(('elapsed_overhead_percent', pair_info['elapsed_overhead_percent']))
            for kind, stat in entries:
                interval = stat['bootstrap95'] or [None, None]
                writer.writerow({'from': pair_info['from'], 'to': pair_info['to'],
                                 'category': pair_info['category'], 'metric': pair_info['metric'],
                                 'kind': kind, 'n': stat.get('n', pair_info['n']),
                                 'mean': stat['mean'], 'ci_low': interval[0], 'ci_high': interval[1],
                                 'exploratory': pair_info['exploratory'],
                                 'paired_blocks': len(pair_info['blocks']),
                                 'zero_denominator_blocks': ','.join(map(str, pair_info['zero_denominator_blocks']))})
    if plt is not None:
        plots(plt, output, by_variant, groups, paired, cfg['mode'])
    print('Analysis:', output / 'summary.json', '| excluded raw trials:', len(failures))


if __name__ == '__main__':
    main()
