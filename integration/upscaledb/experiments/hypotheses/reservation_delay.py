#!/usr/bin/env python3
"""H2: synthetic reserved-slice diagnostic, never DB throughput. No lock changes."""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import signal
import statistics as stats
import subprocess
import sys

from integration.upscaledb._paths import CORE, ROOT, RUNNER, UPSCALEDB
from integration.upscaledb.core.build import rust_sources_digest
from integration.upscaledb.runner.run_trials import discover_topology
from integration.upscaledb.runner.process_execution import capture_command, utc_now

HERE = Path(__file__).resolve().parent
BACKENDS = ('bridge_mutex', 'fc', 'fc_pq', 'uscl', 'uscl_local_observed')
PAUSES = (0, 50, 500, 5000)
REPETITIONS = 3
TIMEOUT_S = 12


def sha(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def save(path, value):
    with Path(path).open('x') as f:
        json.dump(value, f, indent=2, allow_nan=False)
        f.write('\n')


def load(path):
    return json.loads(Path(path).read_text())


def require(condition, message):
    if not condition:
        raise ValueError(message)


def parsed_trial(record, backend, samples):
    try:
        events = [json.loads(line) for line in record['stdout'].splitlines()]
        setup = next(x for x in events if x['type'] == 'setup')
        complete = next(x for x in events if x['type'] == 'complete')
        rows = [x for x in events if x['type'] == 'sample']
        expected = 2 + 2 * samples
        require(record['returncode'] == 0 and not record['timeout'], 'child failed')
        require(setup['backend'] == backend and setup['samples'] == samples, 'wrong trial')
        require(setup['memory_node_mask'] == 1 and setup['setup_cpu'] == 16, 'setup placement')
        require(complete['actor_cpus'] == [16, 17] and complete['joined_before_destroy'], 'lifetime/affinity')
        require(complete['status'] == 'ok' and complete['calls'] == expected and
                complete['token_sum'] == expected * (expected + 1) // 2, 'oracle')
        require(len(rows) == samples and [x['index'] for x in rows] == list(range(samples)), 'sample count')
        require(sum(x['type'] == 'first_use' for x in events) == 1 and
                sum(x['type'] == 'calibration' for x in events) == 3, 'setup records')
        for row in rows:
            require(row['a_callback_end_marker_ns'] <= row['a_return_ns'] <=
                    row['b_request_ns'] <= row['b_entry_ns'] <= row['b_return_ns'], 'handoff order')
            require(row['prime_end_ns'] - row['prime_start_ns'] >= 20000000, 'credit priming')
        return events, None
    except (ValueError, KeyError, StopIteration, TypeError) as exc:
        return [], str(exc) or type(exc).__name__


def assert_inputs(manifest):
    for name, expected in manifest['files'].items():
        require(sha(name) == expected, f'input changed: {name}')
    require(rust_sources_digest() == manifest['rust_sources_sha256'], 'frozen Rust/C sources changed')


def prepare(args):
    root = args.output_root
    root.mkdir(parents=True, exist_ok=False)
    (root / 'src').mkdir()
    topology = discover_topology()
    chosen = [topology['cpus'].get(str(cpu)) for cpu in (16, 17)]
    require(all(x and x['node'] == 0 for x in chosen), 'CPUs16,17 must be allowed and on node0')
    require((chosen[0]['socket'], chosen[0]['core']) !=
            (chosen[1]['socket'], chosen[1]['core']), 'actors must occupy distinct physical cores')
    os.sched_setaffinity(0, {16, 17})
    files = {}
    for path in (Path(__file__), HERE / 'reservation_delay.cc', UPSCALEDB / '_paths.py',
                 CORE / 'bridge.h', CORE / 'build.py', RUNNER / 'run_trials.py',
                 RUNNER / 'process_execution.py', ROOT / 'c/u-scl/fairlock.h',
                 ROOT / 'c/u-scl/common.h', ROOT / 'c/u-scl/rdtsc.h'):
        files[str(path.resolve())] = sha(path)
        shutil.copy2(path, root / 'src' / path.name)
    manifests = {}
    for backend in BACKENDS[:-1]:
        path = args.binary_root / f'build-{backend}.json'
        manifests[backend] = load(path)
        files[str(path)] = sha(path)
        shutil.copy2(path, root / path.name)
    frozen = manifests['uscl']
    lib = frozen['rust_staticlib']
    rust_hash = rust_sources_digest()
    require(lib['rust_sources_sha256'] == rust_hash, 'current source differs from frozen library provenance')
    require(sha(CORE / 'bridge.h') == lib['bridge_h_sha256'], 'bridge ABI changed')
    require(sha(lib['archive']) == lib['archive_sha256'], 'frozen archive changed')
    require(sha(ROOT / frozen['baseline']['source']) == frozen['baseline']['sha256'] and
            sha(ROOT / 'c/u-scl/common.h') == frozen['baseline']['common_sha256'], 'USCL source changed')
    for backend, entry in manifests.items():
        require(entry['rust_staticlib'] == lib, f'{backend}: different frozen bridge provenance')
    files[lib['archive']] = lib['archive_sha256']
    for entry in (frozen['toolchain'],):
        for key in ('cc', 'cxx'):
            require(sha(entry[key]) == entry[key + '_sha256'], f'{key} compiler changed')
            files[entry[key]] = entry[key + '_sha256']
    shared = frozen['library_verification']
    require(sha(shared['shared_object']) == shared['shared_object_sha256'], 'frozen DB library changed')
    files[shared['shared_object']] = shared['shared_object_sha256']
    cohort_path = args.cohort or root.parent / 'cohort.json'
    cohort = load(cohort_path)
    require(cohort['partitions']['H2']['cpus'] == [16, 17] and
            cohort['partitions']['H2']['memory_node'] == 0, 'cohort H2 placement mismatch')
    files[str(cohort_path)] = sha(cohort_path)
    shutil.copy2(cohort_path, root / 'cohort.json')

    source = root / 'src/reservation_delay.cc'
    bridge_binary, direct_binary = root / 'reservation-bridge', root / 'reservation-uscl-local'
    # Preserve the ACTUAL compiler, compile flags and complete link arguments.
    # Only source and -o target change. This is a synthetic callback driver;
    # linking libupscaledb does not make bridge_mutex a native DB workload.
    command = list(frozen['build']['harness_command'])
    old_source = str(CORE / 'native_harness.cc')
    require(command.count(old_source) == 1, 'unexpected frozen harness command')
    command[command.index(old_source)] = str(source)
    command[command.index('-o') + 1] = str(bridge_binary)
    # Same .cc file's C branch includes the unmodified, copied local USCL header.
    # It is NOT linked into the frozen bridge executable and cannot interpose it.
    cc = frozen['toolchain']['cc']
    cxx = frozen['toolchain']['cxx']
    object_file = root / 'reservation-uscl.o'
    commands = [command,
                [cc, '-x', 'c', '-std=gnu11', '-O3', '-g', '-DNDEBUG', '-pthread',
                 '-I' + str(root / 'src'), '-c', str(source), '-o', str(object_file)],
                [cxx, '-std=c++17', '-O3', '-g', '-DNDEBUG', '-pthread', '-DH2_DIRECT',
                 '-I' + str(root / 'src'), str(source), str(object_file), '-o', str(direct_binary)]]
    env = os.environ.copy()
    env.pop('NIX_CFLAGS_COMPILE', None)
    env.pop('NIX_LDFLAGS', None)
    for i, cmd in enumerate(commands):
        result = capture_command(cmd, 120, cwd=ROOT, env=env,
                                 metadata={'parent_affinity': sorted(os.sched_getaffinity(0))})
        save(root / f'compile-{i}.json', result)
        require(result['returncode'] == 0 and not result['timeout'], f'compile-{i} failed; log retained')
    for path in (bridge_binary, direct_binary, object_file, *(root / 'src').iterdir()):
        files[str(path)] = sha(path)
    smoke_results = []
    for backend in BACKENDS:
        binary = direct_binary if backend == 'uscl_local_observed' else bridge_binary
        smoke = capture_command([binary, backend, '50', '2'], TIMEOUT_S, cwd=ROOT,
                                metadata={'parent_affinity': sorted(os.sched_getaffinity(0))})
        _, error = parsed_trial(smoke, backend, 2)
        smoke['validation_error'] = error
        path = root / f'smoke-{backend}.json'
        save(path, smoke)
        smoke_results.append({'backend': backend, 'file': path.name, 'sha256': sha(path), 'error': error})
    require(all(x['error'] is None for x in smoke_results), 'correctness smoke failed; all evidence retained')
    manifest = {'schema': 1, 'hypothesis': 'H2 synthetic reservation, NOT database throughput',
                'created_utc': utc_now(), 'cohort_label': args.cohort_label,
                'cohort_sha256': sha(cohort_path), 'topology': topology,
                'actor_cpus': [16, 17], 'setup_cpu': 16, 'memory_policy': 'bind node0',
                'backends': BACKENDS, 'pauses_us': PAUSES, 'samples': args.samples,
                'repetitions': REPETITIONS, 'seed': args.seed, 'trial_count': 60,
                'bridge_binary': str(bridge_binary), 'direct_binary': str(direct_binary),
                'files': files, 'rust_sources_sha256': rust_hash, 'build_commands': commands,
                'smokes': smoke_results, 'frozen_uscl_baseline': frozen['baseline'],
                'timeout_s': TIMEOUT_S, 'prime_ns': 20000000,
                'notes': ['Three fresh-process repetitions per cell; inner samples are correlated.',
                          'A never reenters while B is pending; both actors stay alive through B return.',
                          'Fixed first-use calls A then B, then 20ms elapsed credit before EVERY sample.',
                          'No retry/filter for slice validity. Direct snapshots are exclusive, not live polling.',
                          'Bridge internals opaque: mechanism inference only for frozen uscl.',
                          'uscl_local_observed recompiles unchanged local USCL, not the frozen Rust bridge.',
                          'Main co-run shares socket0 LLC/power with H1; disjoint cores are not isolation.',
                          'Zero pause means immediate B request after notification, not zero observed pause.',
                          'Pause is release-to-request target; A remains outside until B completes.',
                          'Calibration: 3 requested20ms sleeps with actual TSC/monotonic delta, 1000 clock pairs.',
                          'No tuning, continuous reentry control, DB work, new lock, or futex modification.']}
    assert_inputs(manifest)
    save(root / 'prepare.json', manifest)
    print(f'Prepared {root}: 5 backends x 4 pauses x 3 processes = 60 trials; {args.samples} samples each')


def run(args):
    root = args.output_root
    manifest = load(root / 'prepare.json')
    assert_inputs(manifest)
    for smoke in manifest['smokes']:
        require(sha(root / smoke['file']) == smoke['sha256'] and smoke['error'] is None, 'smoke provenance')
    require({16, 17} <= os.sched_getaffinity(0), 'actors no longer allowed')
    os.sched_setaffinity(0, {16, 17})
    raw = root / 'trials'
    raw.mkdir(exist_ok=False)
    schedule = []
    rng = random.Random(manifest['seed'])
    for repetition in range(REPETITIONS):
        cells = [(backend, pause) for backend in BACKENDS for pause in PAUSES]
        rng.shuffle(cells)
        schedule += [{'repetition': repetition, 'backend': backend, 'pause_us': pause}
                     for backend, pause in cells]
    save(root / 'schedule.json', schedule)
    run_record = {'start_utc': utc_now(), 'cohort_label': manifest['cohort_label'],
                  'prepare_sha256': sha(root / 'prepare.json'),
                  'schedule_sha256': sha(root / 'schedule.json'), 'trials': []}
    for index, cell in enumerate(schedule):
        binary = manifest['direct_binary' if cell['backend'] == 'uscl_local_observed' else 'bridge_binary']
        trial = capture_command([binary, cell['backend'], str(cell['pause_us']),
                                 str(manifest['samples'])], TIMEOUT_S, cwd=ROOT,
                                metadata={'parent_affinity': sorted(os.sched_getaffinity(0))})
        _, error = parsed_trial(trial, cell['backend'], manifest['samples'])
        trial.update(cell, validation_error=error)
        path = raw / f'{index:03d}.json'
        save(path, trial)
        run_record['trials'].append({**cell, 'file': str(path.relative_to(root)),
                                     'sha256': sha(path), 'validation_error': error})
    run_record['end_utc'] = utc_now()
    save(root / 'run.json', run_record)
    assert_inputs(manifest)
    failures = sum(x['validation_error'] is not None for x in run_record['trials'])
    print(f'Timed stage complete: {len(schedule)} fresh processes; failures={failures}; no analysis executed')
    return bool(failures)


def write_csv(path, rows):
    with path.open('x', newline='') as f:
        if rows:
            writer = csv.DictWriter(f, fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)


def analyze(args):
    root = args.output_root
    manifest, run_record = load(root / 'prepare.json'), load(root / 'run.json')
    require(sha(root / 'prepare.json') == run_record['prepare_sha256'], 'prepare manifest changed')
    require(sha(root / 'schedule.json') == run_record['schedule_sha256'], 'schedule changed')
    schedule = load(root / 'schedule.json')
    require(len(schedule) == len(run_record['trials']) == 60, 'incomplete schedule')
    out = args.analysis_root or root / 'analysis'
    out.mkdir(parents=True, exist_ok=False)
    rows, trials, errors = [], [], []
    for expected, item in zip(schedule, run_record['trials']):
        require(all(item[k] == v for k, v in expected.items()), 'schedule identity changed')
        require(sha(root / item['file']) == item['sha256'], 'raw trial changed')
        record = load(root / item['file'])
        events, error = parsed_trial(record, item['backend'], manifest['samples'])
        identity = {k: item[k] for k in ('backend', 'pause_us', 'repetition')}
        if error:
            errors.append({**identity, 'file': item['file'], 'error': error,
                           'timeout': record['timeout'], 'returncode': record['returncode']})
            continue
        rates = [x['tsc_ticks'] / x['elapsed_ns'] for x in events if x['type'] == 'calibration']
        rate = stats.median(rates)
        first = next(x for x in events if x['type'] == 'first_use')
        sample_rows = []
        for sample in (x for x in events if x['type'] == 'sample'):
            observed = bool(sample['b_observed'])
            unbanned = not sample['b_banned'] or sample['b_banned_until_tsc'] <= sample['b_request_tsc']
            owned = (sample['a_slice_valid'] and sample['b_slice_valid'] and
                     sample['a_slice_tsc'] == sample['b_slice_tsc'] == sample['a_own_slice_tsc'] and
                     sample['b_own_slice_tsc'] != sample['b_slice_tsc'] and
                     sample['a_total_weight'] == sample['b_total_weight'] == 2048)
            remaining = max(0, sample['b_slice_tsc'] - sample['b_request_tsc']) / rate if observed else None
            eligible = bool(observed and owned and unbanned and remaining > 0)
            delay = sample['b_entry_ns'] - sample['b_request_ns']
            row = {**identity, 'index': sample['index'], 'delay_ns': delay,
                   'release_to_request_ns': sample['b_request_ns'] - sample['a_return_ns'],
                   'release_to_entry_ns': sample['b_entry_ns'] - sample['a_return_ns'],
                   'release_to_b_return_ns': sample['b_return_ns'] - sample['a_return_ns'],
                   'prime_actual_ns': sample['prime_end_ns'] - sample['prime_start_ns'],
                   'pause_loops': sample['pause_loops'], 'observed': observed,
                   'a_retained_valid': bool(sample['a_slice_valid']) if observed else None,
                   'b_unbanned': bool(unbanned) if observed else None,
                   'eligible_remaining_reservation': eligible,
                   'remaining_reservation_ns': remaining,
                   'delay_minus_remaining_ns': delay - remaining if observed else None,
                   'entry_minus_slice_ticks': sample['b_entry_tsc'] - sample['b_slice_tsc'] if observed else None,
                   'tsc_ticks_per_ns': rate}
            rows.append(row)
            sample_rows.append(row)
        eligible_rows = [x for x in sample_rows if x['eligible_remaining_reservation']]
        # Declared descriptive criterion, not an OS-causality test or significance test.
        consistent = sum(-10000 <= x['delay_minus_remaining_ns'] <= 250000 for x in eligible_rows)
        trials.append({**identity, 'samples': len(sample_rows),
                       'delay_median_ns': stats.median(x['delay_ns'] for x in sample_rows),
                       'delay_min_ns': min(x['delay_ns'] for x in sample_rows),
                       'delay_max_ns': max(x['delay_ns'] for x in sample_rows),
                       'pause_actual_median_ns': stats.median(x['release_to_request_ns'] for x in sample_rows),
                       'eligible_samples': len(eligible_rows), 'consistent_samples': consistent,
                       'first_a_delay_ns': first['a_entry_ns'] - first['a_request_ns'],
                       'first_b_delay_ns': first['b_entry_ns'] - first['b_request_ns'],
                       'tsc_ticks_per_ns': rate, 'calibration_min_rate': min(rates),
                       'calibration_max_rate': max(rates)})
    groups = []
    for backend in BACKENDS:
        for pause in PAUSES:
            values = [x['delay_median_ns'] for x in trials if x['backend'] == backend and x['pause_us'] == pause]
            groups.append({'backend': backend, 'pause_us': pause, 'fresh_processes': len(values),
                           'median_of_process_medians_ns': stats.median(values) if values else None,
                           'min_process_median_ns': min(values) if values else None,
                           'max_process_median_ns': max(values) if values else None})
    direct = [x for x in trials if x['backend'] == 'uscl_local_observed' and x['pause_us'] < 5000]
    direct_expired = [x for x in trials if x['backend'] == 'uscl_local_observed' and x['pause_us'] == 5000]
    if errors or len(direct) != 9 or any(x['eligible_samples'] == 0 for x in direct):
        classification = 'inconclusive'
    elif (all(x['eligible_samples'] == manifest['samples'] and
              x['consistent_samples'] >= 0.9 * x['eligible_samples'] for x in direct) and
          len(direct_expired) == 3 and all(x['eligible_samples'] == 0 and x['delay_median_ns'] < 250000
                                        for x in direct_expired)):
        classification = 'supported (observed local mechanism only; frozen bridge remains inference)'
    else:
        classification = 'mixed (inspect validity, bans, actual pauses and scheduler residuals)'
    summary = {'classification': classification, 'failures': errors, 'trial_summaries': trials,
               'groups': groups, 'inner_samples_are_independent_repetitions': False,
               'criterion': 'Every short-pause direct trial: all samples have observed A-owned valid slice, B not banned, '
                            'positive remaining time; >=90% delay-minus-remaining in [-10us,250us]. '
                            'Every 5ms direct trial: no positive remaining slice, median delay<250us. '
                            'This is a declared descriptive screen, not significance or OS-causality proof.',
               'run_sha256': sha(root / 'run.json')}
    save(out / 'summary.json', summary)
    write_csv(out / 'samples.csv', rows)
    write_csv(out / 'trials.csv', trials)
    write_csv(out / 'summary.csv', groups)
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    fig, ax = plt.subplots(figsize=(9, 5))
    for bindex, backend in enumerate(BACKENDS):
        selected = [x for x in trials if x['backend'] == backend]
        ax.scatter([PAUSES.index(x['pause_us']) + (bindex-2)*0.12 for x in selected],
                   [x['delay_median_ns']/1000 for x in selected], label=backend, alpha=0.8)
    ax.set_xticks(range(4), ['0', '50', '500', '5000'])
    ax.set_xlabel('Requested completed-release to B-request pause (us)')
    ax.set_ylabel('B call-to-callback delay (us); one median per fresh process')
    ax.set_yscale('symlog', linthresh=10)
    ax.set_title('H2 synthetic handoff: 3 fresh processes/cell, not DB throughput')
    ax.legend(fontsize=8)
    fig.tight_layout(); fig.savefig(out / 'process_delays.png', dpi=160); plt.close(fig)
    fig, ax = plt.subplots(figsize=(7, 5))
    observed_rows = [x for x in rows if x['observed']]
    for pause in PAUSES:
        selected = [x for x in observed_rows if x['pause_us'] == pause]
        ax.scatter([x['remaining_reservation_ns']/1000 for x in selected],
                   [x['delay_ns']/1000 for x in selected], label=f'{pause}us', s=16, alpha=0.6)
    bound = max([x['remaining_reservation_ns']/1000 for x in observed_rows] + [1])
    ax.plot([0, bound], [0, bound], '--', color='black', label='delay = remaining')
    ax.set_xlabel('Exclusive pre-request USCL snapshot: remaining slice (us)')
    ax.set_ylabel('B call-to-callback delay (us)')
    ax.set_title('Unchanged local USCL companion; ALL inner samples (correlated)')
    ax.legend(); fig.tight_layout(); fig.savefig(out / 'remaining_vs_delay.png', dpi=160); plt.close(fig)
    lines = ['# H2 reserved-slice diagnostic', '', f'Classification: **{classification}**.', '',
             f"60 scheduled fresh processes; {len(trials)} successful, {len(errors)} failed; "
             f"{manifest['samples']} correlated inner samples/process. Three independent process repetitions/cell; "
             'no confidence intervals or DB throughput claim.', '',
             '## Numeric results', '',
             '| Backend | Requested pause us | Processes | Median process delay us | Observed min-max us |',
             '|---|---:|---:|---:|---:|']
    for group in groups:
        values = [group[k] for k in ('median_of_process_medians_ns', 'min_process_median_ns', 'max_process_median_ns')]
        text = [f'{v/1000:.3f}' if v is not None else 'NA' for v in values]
        lines.append(f"| {group['backend']} | {group['pause_us']} | {group['fresh_processes']} | {text[0]} | {text[1]}–{text[2]} |")
    lines += ['', '## Protocol, provenance and limits', '', *['- ' + x for x in manifest['notes']],
              '- A callback-end marker precedes return; A call-return is a conservative completed-release timestamp. '
              'B request follows an acquire/release atomic notification. No callback is active between those events.',
              '- Snapshots are read only while A waits outside the API and before B enters. A/B remain alive; '
              'both are joined before destruction. Snapshot overhead is included, not subtracted.',
              '- The first A/B calls include TLS/registration and are reported separately in trials.csv; '
              'they are never pooled with measured samples. B first-use can also wait on A\'s first reservation.',
              '- Callback oracle checks non-overlap, exactly one execution/result per call, total count and token sum; '
              'each successful process reports verified singleton actor affinity and node0 memory policy.',
              '- Original local USCL retains a slice unless release marks the owner banned. '
              'B\'s own ban is separately recorded to avoid mistaking credit throttling for retained ownership. '
              'No successful-only selection: samples.csv contains every sample from each successful process; '
              'failed/partial stdout remains in raw trial JSON and failures are enumerated in summary.json.',
              '- Slice ticks are converted with each process\'s three actual 20ms TSC/monotonic calibrations. '
              'Cross-CPU invariant/synchronized TSC is an assumption, not a scheduler guarantee. '
              'No timing overhead subtraction. Raw timestamps and exact pause loop counts are retained.',
              '- Existing USCL uses nominal 4,800,000 ticks (2400 ticks/us compile assumption), not calibrated 2ms. '
              'Its futex expected-value path may return EAGAIN; no claim that the waiting competitor truly sleeps.',
              '- The direct companion is a separate executable compiled from unchanged source, without Rust bridge. '
              'Direct state/mechanism evidence does not prove identical frozen-bridge internals or production DB behavior. '
              'Bridge-mutex means a pthread mutex borrowed by the bridge, never native DB.',
              '- Delay tracking observed remaining reservation is stronger than delay alone, but descheduling, '
              'clock brackets and polling still add residuals. A retained valid bit past expiry is not remaining time. '
              'Work-conserving controls can incur OS delay too. Repeat decisive contrasts serially.',
              '- Requested pause labels describe when B arrives, not an instruction for A to reenter. '
              'A stays idle through B return; actual release-to-request and release-to-return durations are recorded.',
              '', 'Descriptive screen: ' + summary['criterion'], '',
              'See summary.json for every failure, trials.csv for all process medians/first-use/calibration ranges, '
              'samples.csv for all delays/actual pauses/validity counts. All commands, source/binary/library '
              'SHA-256 identities, smoke records, UTC intervals, topology and cohort label are in prepare.json and raw JSON.',
              '', '![Fresh-process delays](process_delays.png)', '', '![Remaining reservation](remaining_vs_delay.png)', '']
    (out / 'report.md').write_text('\n'.join(lines))
    print(f'Analysis: {out}; {classification}; failures={len(errors)}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--prepare-only', action='store_true')
    mode.add_argument('--run', action='store_true')
    mode.add_argument('--analyze-only', action='store_true')
    parser.add_argument('--output-root', type=Path, required=True)
    parser.add_argument('--binary-root', type=Path, default=ROOT / '.worktree/upscaledb-joined-build')
    parser.add_argument('--cohort', type=Path)
    parser.add_argument('--cohort-label', default='partitioned-parallel-H1-H2-H3')
    parser.add_argument('--samples', type=int, default=12, help='prepare only, bounded 1..32')
    parser.add_argument('--seed', type=int, default=241902, help='prepare only')
    parser.add_argument('--analysis-root', type=Path, help='new directory, default OUTPUT/analysis')
    args = parser.parse_args()
    args.output_root = args.output_root.resolve()
    args.binary_root = args.binary_root.resolve()
    if args.cohort:
        args.cohort = args.cohort.resolve()
    require(1 <= args.samples <= 32, 'samples must be 1..32')
    # A terminal SIGTERM must clean up a verified child, not orphan a hung lock.
    def stop(signum, frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, stop)
    if args.prepare_only:
        prepare(args)
    elif args.run:
        return int(run(args))
    else:
        analyze(args)
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        sys.exit(f'H2 error (existing evidence retained): {exc}')
