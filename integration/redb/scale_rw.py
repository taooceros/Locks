#!/usr/bin/env python3
"""Client scalability (1-64 writers, socket boundary) and reader/writer campaign.

Extends the redb write-path trials (run.py) in two directions:

  scale   W writer clients, W in 1,2,4,8,16,32 on one socket (node 0, one per
          physical core, CPUs 0..W-1) and 64 across both sockets (CPUs 0-31 on
          node 0 plus 32-63 on node 1). Cohorts all1 and half1_half64, durability
          None, all variants. Cells with CPUs on both nodes are marked cross_socket.
  rw      W writers plus R reader threads on separate physical cores of node 0
          (CPUs W.. for the readers of the forward view, fixed 8..8+R-1 otherwise):
          all1, None; forward W=8 with R in 0,1,4,8,16, reverse R=8 with W in
          1,2,4,8. A reader runs read transactions (begin_read, point gets, a short
          scan; main.rs `reader`) outside every write lock for the whole window.
  perf    the cache-counter cohort under perf stat: mcs/fc/fc_pq at W=32 and W=64
          (all1 and half1_half64) and all variants at W=8 with R=0 and R=16.

Memory: every cell runs under `numactl --membind=<node>` (node 0 by default), so
only the CPUs move at W>32: the extra 32 clients run on node 1 against node-0
memory. The runner itself is pinned to an idle SMT sibling outside every client
CPU. The measurement lock is taken exclusively per (cohort, W, R) group of
variants, not for the whole campaign, so other measurement jobs can interleave
between groups. Every cell records the per-CPU clock (sampled as in run.py) and is
flagged when outside F +- 2 % (S1/S2): this host cannot be assumed to hold the
fixed clock on all 64 cores.

Usage (repository root; under a shared MEASUREMENT_LOCK for prepare/analyze):
  python3 -m integration.redb.scale_rw --prepare-only --build-dir B --output-root R --power-setup S1
  python3 -m integration.redb.scale_rw --run {scale,rw,perf} --output-root R
  python3 -m integration.redb.scale_rw --analyze {scale,rw,perf} --output-root R
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import random
import shutil
import statistics
import subprocess
import sys
import time

from integration.redb import run as base
from integration.redb.build import OUT, load_build

VARIANTS = base.VARIANTS
COHORTS = ('all1', 'half1_half64')
SCALE_W = (1, 2, 4, 8, 16, 32, 64)
RW_FORWARD_R = (0, 1, 4, 8, 16)
RW_REVERSE_W = (1, 2, 4, 8)
RW_REVERSE_R = 8
PERF_WIDE_W = (32, 64)
PERF_WIDE_VARIANTS = ('mcs', 'fc', 'fc_pq')
PERF_RW_R = (0, 16)
MAX_WORKERS = 64
MAX_RECORDS = MAX_WORKERS * base.MAX_RECORDS_PER_WORKER
RUNNER_CPU = 127
DURABILITY = 'none'
SCHEMA = 'scale-rw-1'
KINDS = ('scale', 'rw', 'perf')
SEEDS = base.SEEDS
FINE_SUB = 8


def lock_path():
    return Path(os.environ.get('MEASUREMENT_LOCK', Path.home() / '.cache/locks-experiments/measurement.lock'))


class MeasurementLock:
    """Exclusive flock on the shared measurement lock; blocks until the other jobs release it."""

    def __enter__(self):
        path = lock_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        self.stream = path.open('a')
        begin = time.monotonic()
        fcntl.flock(self.stream, fcntl.LOCK_EX)
        self.waited = time.monotonic() - begin
        return self

    def __exit__(self, *exc):
        fcntl.flock(self.stream, fcntl.LOCK_UN)
        self.stream.close()


def physical_cpus():
    """One CPU per physical core, node 0 first: [(cpu, node)], lowest SMT id per core."""
    cores = {}
    for line in subprocess.check_output(['lscpu', '-p=CPU,CORE,SOCKET,NODE'], text=True).splitlines():
        if line.startswith('#'):
            continue
        cpu, core, socket, node = (int(x) for x in line.split(','))
        key = (socket, core)
        if key not in cores or cpu < cores[key][0]:
            cores[key] = (cpu, node)
    return sorted(cores.values(), key=lambda entry: (entry[1], entry[0]))


def cell_cpus(physical, writers, readers, reader_first):
    """Writer CPUs (first W physical cores) and reader CPUs (physical cores reader_first..)."""
    if writers + 0 > len(physical) or reader_first + readers > len(physical):
        raise RuntimeError('not enough physical cores for the cell')
    writer_cpus = [cpu for cpu, _ in physical[:writers]]
    reader_cpus = [cpu for cpu, _ in physical[reader_first:reader_first + readers]]
    if set(writer_cpus) & set(reader_cpus):
        raise RuntimeError('reader CPUs overlap writer CPUs')
    return writer_cpus, reader_cpus


def reader_first(writers):
    """Readers start at physical core 8 (W <= 8) or right after the writers."""
    return max(8, writers)


def matrix(identity, kind):
    """Groups of the campaign: dicts (cohort, writers, readers, variants, repeats)."""
    variants = [v for v in VARIANTS if v in identity['variants']]
    physical = identity['physical_cpus']
    socket_size = identity['socket_cores']
    groups = []

    def add(cohort, writers, readers, chosen=None):
        chosen = [v for v in (chosen or variants) if v in variants]
        if not chosen:
            return
        cross = writers > socket_size
        repeats = identity['repeats_cross_socket'] if cross else identity['repeats']
        key = (cohort, writers, readers)
        if all((g['cohort'], g['writers'], g['readers']) != key for g in groups):
            groups.append({'cohort': cohort, 'writers': writers, 'readers': readers,
                           'variants': chosen, 'repeats': repeats})

    if kind == 'scale':
        for cohort in COHORTS:
            for writers in SCALE_W:
                if writers <= len(physical):
                    add(cohort, writers, 0)
    elif kind == 'rw':
        for readers in RW_FORWARD_R:
            add('all1', 8, readers)
        for writers in RW_REVERSE_W:
            add('all1', writers, RW_REVERSE_R)
    elif kind == 'perf':
        for cohort in COHORTS:
            for writers in PERF_WIDE_W:
                if writers <= len(physical):
                    add(cohort, writers, 0, PERF_WIDE_VARIANTS)
        for readers in PERF_RW_R:
            add('all1', 8, readers)
    else:
        raise ValueError(kind)
    return groups


def cells(identity, kind):
    """(repeat, group) in execution order: repeats outermost."""
    groups = matrix(identity, kind)
    return [(repeat, group) for repeat in range(max(g['repeats'] for g in groups))
            for group in groups if repeat < group['repeats']]


def cell_name(repeat, group, variant):
    return f"r{repeat}_{group['cohort']}_w{group['writers']}_r{group['readers']}_{DURABILITY}_{variant}"


def expected_cells(identity, kind):
    return sum(len(group['variants']) for _, group in cells(identity, kind))


def prepare(root, build_dir, numa_node, power_setup, fixed_ghz, variants, repeats, repeats_cross_socket):
    physical = physical_cpus()
    if len(physical) < MAX_WORKERS:
        raise RuntimeError(f'need {MAX_WORKERS} physical cores, host has {len(physical)}')
    physical = physical[:MAX_WORKERS]
    nodes = sorted({node for _, node in physical})
    socket_size = sum(1 for _, node in physical if node == nodes[0])
    all_cpus = [cpu for cpu, _ in physical]
    if RUNNER_CPU in all_cpus:
        raise RuntimeError('runner CPU overlaps the client CPUs')
    power = base.power_preflight(all_cpus, power_setup, fixed_ghz)
    build = load_build(build_dir)
    if not build['instrumented']:
        raise RuntimeError('uninstrumented builds are for the overhead check only')
    if 'fcpq_fast_path' not in build['binaries']['patched']['resolved_features'].get('libdlock', []):
        raise RuntimeError('FC-PQ must be built with fcpq_fast_path')
    root.mkdir(parents=True, exist_ok=False)
    binaries = root / 'binaries'
    binaries.mkdir()
    snapshots = {}
    for name, entry in build['binaries'].items():
        if name == 'test_hooks':
            continue
        snapshot = binaries / name
        shutil.copy2(build_dir / entry['file'], snapshot)
        snapshot.chmod(0o555)
        if base.digest(snapshot) != entry['sha256']:
            raise RuntimeError(f'snapshot of {name} differs from build manifest')
        snapshots[name] = {'file': str(snapshot.relative_to(root)), 'sha256': entry['sha256'],
                           'features': entry['features'], 'resolved_features': entry['resolved_features'],
                           'build_command': entry['command']}
    identity = {
        'schema': SCHEMA,
        'build_dir': str(build_dir),
        'redb_source': build['redb_source'],
        'harness_sources_sha256': build['harness_sources_sha256'],
        'runner_sha256': {'scale_rw.py': base.digest(Path(__file__).resolve()),
                          'run.py': base.digest(Path(base.__file__).resolve())},
        # jj workspaces are not colocated (git would report another workspace): record jj's own view.
        'jj_parent_commit': subprocess.check_output(['jj', 'log', '-r', '@-', '--no-graph', '-T', 'commit_id'],
                                                    cwd=base.HERE, text=True).strip(),
        'jj_working_copy_commit': subprocess.check_output(['jj', 'log', '-r', '@', '--no-graph', '-T', 'commit_id'],
                                                          cwd=base.HERE, text=True).strip(),
        'rustc': build['rustc'], 'binaries': snapshots, 'variant_binary': build['variant_binary'],
        'physical_cpus': physical, 'socket_cores': socket_size, 'runner_cpu': RUNNER_CPU,
        'placement': base.placement(all_cpus, numa_node),
        'numa_memory_node': numa_node,
        'memory_policy': ('numactl --membind=%d for every cell (node-%d memory also for the clients on the other '
                          'socket)' % (numa_node, numa_node)) if numa_node is not None else 'unbound',
        'variants': list(variants), 'repeats': repeats, 'repeats_cross_socket': repeats_cross_socket,
        'perf': base.perf_identity(), 'tsc': base.tsc_identity(), 'power': power,
        'frequency_sampling': {'kinds': list(base.FREQ_SAMPLE_KINDS), 'interval_s': base.FREQ_SAMPLE_SECONDS,
                               'busy_every': base.FREQ_BUSY_EVERY, 'busy_min': base.FREQ_BUSY_MIN,
                               'min_samples': base.FREQ_MIN_SAMPLES, 'mixed_ratio': base.FREQ_MIXED_RATIO},
        'cpus': all_cpus,
        'filesystem': subprocess.check_output(['findmnt', '-n', '-o', 'SOURCE,FSTYPE,OPTIONS', '-T', str(root)],
                                              text=True).strip(),
        'max_records_per_trial': MAX_RECORDS, 'max_records_per_worker': base.MAX_RECORDS_PER_WORKER,
        'max_db_file_bytes': base.DB_FILE_LIMIT, 'duration_ms': base.DURATION_MS,
        'time_limit_seconds': base.TRIAL_TIMEOUT_SECONDS, 'durability': DURABILITY,
        'reader': {'gets_per_txn': 4, 'scan_entries': 16, 'stale_every': 16,
                   'note': 'read txn = begin_read + last key of a random writer that has data (one discovery probe of a random writer every 64th txn) + 4 point gets + '
                           'scan of 16 consecutive keys, all verified exactly; begin_read/txn latency in '
                           'log2 x 8 histograms; staleness = records committed between the snapshot and a second '
                           'begin_read after the txn (every 16th txn)'},
        'boundary_note': 'Critical section = one whole write transaction submitted as a closure; reads never enter any write lock',
        'seeds': SEEDS,
    }
    identity['matrix'] = {kind: [{**g, 'cells': len(g['variants']) * g['repeats']} for g in matrix(identity, kind)]
                          for kind in KINDS}
    base.write_json(root / 'manifest.json', identity)
    return identity


def load_identity(root):
    identity = json.loads((root / 'manifest.json').read_text())
    if identity.get('schema') != SCHEMA:
        raise RuntimeError('not a scale_rw manifest')
    build = load_build(Path(identity['build_dir']))
    for key in ('redb_source', 'harness_sources_sha256'):
        if build[key] != identity[key]:
            raise RuntimeError(f'{key} changed after preparation; create a fresh output root')
    now = {'scale_rw.py': base.digest(Path(__file__).resolve()), 'run.py': base.digest(Path(base.__file__).resolve())}
    if identity['runner_sha256'] != now:
        raise RuntimeError('runner changed after preparation; create a fresh output root')
    for name, entry in identity['binaries'].items():
        if base.digest(root / entry['file']) != entry['sha256']:
            raise RuntimeError(f'prepared binary changed: {name}')
    return identity


def cpu_arg(cpus):
    return ','.join(map(str, cpus))


def trial(root, identity, kind, group, repeat, variant):
    name = cell_name(repeat, group, variant)
    directory = root / kind / name
    cohort, writers, readers = group['cohort'], group['writers'], group['readers']
    physical = [tuple(x) for x in identity['physical_cpus']]
    writer_cpus, reader_cpus = cell_cpus(physical, writers, readers, reader_first(writers))
    nodes = sorted({node for cpu, node in physical if cpu in writer_cpus + reader_cpus})
    writer_nodes = sorted({node for cpu, node in physical if cpu in writer_cpus})
    seed = SEEDS[repeat]
    binary_name = identity['variant_binary'][variant]
    binary = root / identity['binaries'][binary_name]['file']
    all_cpus = writer_cpus + reader_cpus
    command = [str(binary), '--database', str(directory / 'db.redb'), '--variant', variant, '--cohort', cohort,
               '--durability', DURABILITY, '--seed', str(seed), '--duration-ms', str(base.DURATION_MS),
               '--cpus', cpu_arg(writer_cpus)]
    if reader_cpus:
        command += ['--reader-cpus', cpu_arg(reader_cpus)]
    if identity['numa_memory_node'] is not None:
        command = ['numactl', f"--membind={identity['numa_memory_node']}", *command]
    env = None
    perf = kind == 'perf'
    if perf:
        control = root / 'perf-control' / name
        control.mkdir(parents=True)
        ctl, ack = control / 'ctl', control / 'ack'
        os.mkfifo(ctl)
        os.mkfifo(ack)
        command = [identity['perf']['path'], 'stat', '-x,', '-o', str(directory / 'perf.csv'), '--delay=-1',
                   '--control', f'fifo:{ctl},{ack}', '-e', ','.join(base.PERF_EVENTS.values()), '--', *command]
        env = {'REDB_PERF_CONTROL': f'{ctl},{ack}'}
    command = ['taskset', '-c', cpu_arg(all_cpus), *command]
    loadavg = os.getloadavg()
    sampler = base.FrequencySampler(all_cpus)
    sampler.start()
    entry = base.command_capture(command, directory, timeout=base.TRIAL_TIMEOUT_SECONDS, limit_db=True, env=env)
    entry['frequency_samples'] = sampler.stop()
    power = identity['power']
    entry['power_problems'] = base.check_power(base.power_state(identity['cpus']), power['setup'], power['fixed_ghz'])
    entry.update({'kind': kind, 'cohort': cohort, 'clients': writers, 'writers': writers, 'readers': readers,
                  'durability': DURABILITY, 'variant': variant, 'repeat': repeat, 'seed': seed, 'binary': binary_name,
                  'binary_sha256': identity['binaries'][binary_name]['sha256'], 'writer_cpus': writer_cpus,
                  'reader_cpus': reader_cpus, 'nodes': nodes, 'cross_socket': len(writer_nodes) > 1,
                  'loadavg_before': loadavg})
    db_file = directory / 'db.redb'
    if db_file.exists():
        entry['database_bytes'] = db_file.stat().st_size
        entry['database_sha256'] = base.digest(db_file)
    if entry['exit_code'] == 0 and not entry['timed_out']:
        try:
            if entry['power_problems']:
                raise ValueError(f"power setup {power['setup']} changed during the cell: {entry['power_problems'][:3]}")
            result = json.loads((directory / 'stdout').read_text())
            if (result['variant'], result['cohort'], result['durability'], result['seed'], result['smoke_transactions'],
                    result['clients'], result['cpus'], len(result['workers']), result['reader_cpus'],
                    len(result['readers'])) != (variant, cohort, DURABILITY, seed, None, writers, writer_cpus,
                                                writers, reader_cpus, readers):
                raise ValueError('result identity mismatch')
            if not result['build']['service_time'] or (variant == 'fc_pq' and not result['build']['fcpq_fast_path']):
                raise ValueError('binary lacks service time or the FC-PQ fast path')
            if (result['max_records'] != MAX_RECORDS or result['max_records_per_worker'] != base.MAX_RECORDS_PER_WORKER
                    or result['verified_live_records'] > MAX_RECORDS
                    or any(w['completed_records'] > base.MAX_RECORDS_PER_WORKER for w in result['workers'])):
                raise ValueError('record capacity guard mismatch')
            if entry.get('database_bytes', 0) > base.DB_FILE_LIMIT:
                raise ValueError('database file capacity guard mismatch')
            # Exact live contents (verify()) are enforced by the trial itself; reopen is required in every cell.
            if not result['reopened_exact']:
                raise ValueError(f"close/reopen mismatch: {result['reopen_error']}")
            if result['verified_live_records'] != sum(w['completed_records'] for w in result['workers']):
                raise ValueError('live records differ from committed records')
            if any(r['read_txns'] == 0 for r in result['readers']):
                raise ValueError('a reader completed no read transaction')
            if result['transfer'] is not None or result['perf_counted'] != perf:
                raise ValueError('transfer check or perf control state mismatch')
            if perf:
                entry['perf_counts'], entry['perf_running'] = base.parse_perf(directory / 'perf.csv')
                multiplexed = {n: r['percent'] for n, r in entry['perf_running'].items() if r['percent'] < 99.99}
                if multiplexed:
                    raise ValueError(f'perf events multiplexed (% running): {multiplexed}')
            entry['results'] = result
        except (ValueError, KeyError, OSError, IndexError) as error:
            entry['parse_error'] = str(error)
    base.write_json(directory / 'result.json', entry)
    if db_file.exists():
        db_file.unlink()
    return 'results' in entry


def run_matrix(root, kind):
    identity = load_identity(root)
    base.power_preflight(identity['cpus'], identity['power']['setup'], identity['power']['fixed_ghz'])
    if kind == 'perf' and not identity.get('perf'):
        raise RuntimeError('perf was not available at preparation')
    if (root / kind).exists():
        raise RuntimeError('refusing to overwrite prior raw trials; use a fresh output root')
    (root / kind).mkdir()
    os.sched_setaffinity(0, {identity['runner_cpu']})
    errors, total, done = [], expected_cells(identity, kind), 0
    begin = time.monotonic()
    for repeat, group in cells(identity, kind):
        order = [v for v in VARIANTS if v in group['variants']]
        cohort_index = COHORTS.index(group['cohort'])
        random.Random(SEEDS[repeat] ^ (cohort_index << 8) ^ (group['writers'] << 24)
                      ^ (group['readers'] << 32)).shuffle(order)
        with MeasurementLock() as lock:
            base.power_preflight(identity['cpus'], identity['power']['setup'], identity['power']['fixed_ghz'])
            for variant in order:
                if not trial(root, identity, kind, group, repeat, variant):
                    errors.append(cell_name(repeat, group, variant))
                done += 1
        print(f"[{kind}] {done}/{total} r{repeat} {group['cohort']} W={group['writers']} R={group['readers']} "
              f"(lock wait {lock.waited:.0f}s, elapsed {time.monotonic() - begin:.0f}s, failed {len(errors)})",
              file=sys.stderr, flush=True)
    if errors:
        raise RuntimeError(f'{len(errors)} failed cells; raw failure evidence retained: {errors}')


# ---- analysis -------------------------------------------------------------------------------------------


def fine_upper_us(index):
    """Inclusive upper bound (us) of a main.rs fine-histogram bucket."""
    if index < 16:
        return (index + 1) / 1000
    shift = (index >> 3) - 1
    return (((8 + (index & 7)) + 1) << shift) / 1000


def fine_quantile(hist, proportion):
    count = sum(hist)
    if not count:
        return None
    threshold = max(1, int(count * proportion + 0.999999))
    total = 0
    for index, n in enumerate(hist):
        total += n
        if total >= threshold:
            return fine_upper_us(index)
    raise AssertionError('invalid histogram')


def reader_metrics(result):
    readers = result['readers']
    if not readers:
        return {}
    seconds = result['duration_ms'] / 1000
    txn = [sum(r['txn_ns_hist'][i] for r in readers) for i in range(len(readers[0]['txn_ns_hist']))]
    begin = [sum(r['begin_ns_hist'][i] for r in readers) for i in range(len(readers[0]['begin_ns_hist']))]
    samples = sum(r['stale_samples'] for r in readers)
    gets, scans = sum(r['gets'] for r in readers), sum(r['scans'] for r in readers)
    return {'reader_txn_s': sum(r['read_txns'] for r in readers) / seconds,
            'reader_ops_s': (gets + scans) / seconds, 'reader_gets_s': gets / seconds,
            'reader_scan_entries_s': sum(r['scan_entries'] for r in readers) / seconds,
            'reader_txn_p50_us': fine_quantile(txn, 0.50), 'reader_txn_p99_us': fine_quantile(txn, 0.99),
            'reader_txn_p999_us': fine_quantile(txn, 0.999),
            'reader_begin_p50_us': fine_quantile(begin, 0.50), 'reader_begin_p99_us': fine_quantile(begin, 0.99),
            'reader_jain': base.jain([r['read_txns'] for r in readers]),
            'reader_stale_mean': sum(r['stale_sum'] for r in readers) / samples if samples else None,
            'reader_stale_max': max(r['stale_max'] for r in readers),
            'reader_stale_nonzero_fraction': sum(r['stale_nonzero'] for r in readers) / samples if samples else None,
            'reader_empty_fraction': sum(r['empty_writer_ranges'] for r in readers)
            / max(1, sum(r['read_txns'] for r in readers))}


def trimmed_clocks(entry, lo, hi):
    """run.client_clocks over a sub-range of the sampled CPUs (writers or readers)."""
    sampling = entry['frequency_samples']
    if hi <= lo:
        return None
    samples = []
    for sample in sampling['samples']:
        item = {'t': sample['t'], 'khz': sample['khz'][lo:hi]}
        if 'jiffies' in sample:
            item['jiffies'] = sample['jiffies'][lo:hi]
        samples.append(item)
    return base.client_clocks({'frequency_samples': {**sampling, 'cpus': sampling['cpus'][lo:hi], 'samples': samples}})


def load_rows(root, kind):
    identity = json.loads((root / 'manifest.json').read_text())
    paths = sorted((root / kind).glob('*/result.json'))
    expected = expected_cells(identity, kind)
    if len(paths) != expected:
        raise RuntimeError(f'expected {expected} {kind} raw results, found {len(paths)}')
    rows = []
    for path in paths:
        entry = json.loads(path.read_text())
        row = {key: entry[key] for key in ('kind', 'cohort', 'writers', 'readers', 'variant', 'repeat', 'seed',
                                           'binary', 'binary_sha256', 'cross_socket', 'nodes', 'writer_cpus',
                                           'reader_cpus', 'loadavg_before')}
        row['clients'] = row['writers']
        row['durability'] = DURABILITY
        row['raw_result'] = str(path.relative_to(root))
        row['failure'] = entry.get('parse_error') or ('timeout' if entry['timed_out'] else
                                                      f"exit {entry['exit_code']}" if entry['exit_code'] != 0 else None)
        if 'results' in entry:
            row.update(base.metrics(entry))
            row.update(reader_metrics(entry['results']))
            if 'perf_counts' in entry:
                row.update(base.perf_metrics(entry))
            writer_clock = trimmed_clocks(entry, 0, row['writers'])
            row.update(writer_clock)
            reader_clock = trimmed_clocks(entry, row['writers'], row['writers'] + row['readers'])
            if reader_clock:
                row['reader_clock_ghz'] = reader_clock['client_clock_ghz']
            row.update(base.clock_metrics(row, identity))
        rows.append(row)
    rows.sort(key=lambda r: (r['cohort'], r['writers'], r['readers'], VARIANTS.index(r['variant']), r['repeat']))
    return identity, rows


SCALE_METRICS = ('throughput_tx_s', 'throughput_records_s', 'service_jain', 'tx_jain', 'long_service_share',
                 'service_utilization', 'process_cpu_seconds', 'clock_ghz', 'client_clock_spread', 'tx_s_at_ref',
                 'response_p99_ms_upper', 'fast_path_hit_rate')
RW_METRICS = SCALE_METRICS + ('reader_txn_s', 'reader_ops_s', 'reader_gets_s', 'reader_scan_entries_s',
                              'reader_txn_p50_us', 'reader_txn_p99_us', 'reader_txn_p999_us', 'reader_begin_p50_us',
                              'reader_begin_p99_us', 'reader_jain', 'reader_stale_mean', 'reader_stale_max',
                              'reader_stale_nonzero_fraction', 'reader_empty_fraction', 'reader_clock_ghz')
PERF_METRICS = base.PERF_TABLE + ('reader_txn_s', 'reader_ops_s', 'ipc')


def table_for(rows, metrics):
    present = {r['variant'] for r in rows}
    keys = sorted({(r['cohort'], r['writers'], r['readers']) for r in rows})
    table = []
    for cohort, writers, readers in keys:
        for variant in (v for v in VARIANTS if v in present):
            chosen = [r for r in rows if not r['failure'] and (r['cohort'], r['writers'], r['readers'], r['variant'])
                      == (cohort, writers, readers, variant)]
            if not chosen:
                continue
            kept = [r for r in chosen if r.get('clock_off_target') is not True]
            table.append({'cohort': cohort, 'writers': writers, 'readers': readers, 'variant': variant,
                          'cross_socket': chosen[0]['cross_socket'],
                          **{m: base.median_range([r.get(m) for r in chosen]) for m in metrics},
                          **base.clock_counts(chosen),
                          'throughput_tx_s_not_flagged': base.median_range([r.get('throughput_tx_s') for r in kept])})
    return table


def fmt(cell, digits, scale=1.0):
    if cell is None:
        return '—'
    return f"{cell['median'] * scale:.{digits}f} [{cell['min'] * scale:.{digits}f}, {cell['max'] * scale:.{digits}f}]"


def markdown(kind, table, clock):
    lines = [f"Power setup {clock['power_setup']}, fixed clock {clock['fixed_ghz']} GHz; off target: "
             f"{clock['clock_off_target_cells']}/{clock['clock_decided_cells']} decided cells "
             f"({clock['clock_unknown_cells']} undecidable).", '']
    if kind == 'scale':
        lines += ['| cohort | W | socket | variant | tx/s | records/s | service_jain | tx_jain | long share | clock GHz | '
                  'off target |', '|' + '---|' * 11]
        for r in table:
            lines.append(f"| {r['cohort']} | {r['writers']} | {'cross' if r['cross_socket'] else 'node0'} | "
                         f"{r['variant']} | {fmt(r['throughput_tx_s'], 0)} | {fmt(r['throughput_records_s'], 0)} | "
                         f"{fmt(r['service_jain'], 3)} | {fmt(r['tx_jain'], 3)} | {fmt(r['long_service_share'], 3)} | "
                         f"{fmt(r['clock_ghz'], 2)} | {r['clock_off_target_cells']}/{r['cells']} |")
    elif kind == 'rw':
        lines += ['| W | R | variant | writer tx/s | service_jain | reader ops/s | txn p50 us | txn p99 us | '
                  'begin p99 us | stale mean | stale max | clock GHz | off target |', '|' + '---|' * 13]
        for r in table:
            lines.append(f"| {r['writers']} | {r['readers']} | {r['variant']} | {fmt(r['throughput_tx_s'], 0)} | "
                         f"{fmt(r['service_jain'], 3)} | {fmt(r.get('reader_ops_s'), 0)} | "
                         f"{fmt(r.get('reader_txn_p50_us'), 1)} | {fmt(r.get('reader_txn_p99_us'), 1)} | "
                         f"{fmt(r.get('reader_begin_p99_us'), 1)} | {fmt(r.get('reader_stale_mean'), 2)} | "
                         f"{fmt(r.get('reader_stale_max'), 0)} | {fmt(r['clock_ghz'], 2)} | "
                         f"{r['clock_off_target_cells']}/{r['cells']} |")
    else:
        lines += ['| cohort | W | R | variant | tx/s | HITM loads/tx | HITM supplied/tx | L2-miss loads/tx | '
                  'LLC miss/tx | instr/tx | cycles/tx | IPC | eff GHz |', '|' + '---|' * 13]
        for r in table:
            lines.append(f"| {r['cohort']} | {r['writers']} | {r['readers']} | {r['variant']} | "
                         f"{fmt(r['throughput_tx_s'], 0)} | {fmt(r['hitm_loads_per_tx'], 0)} | "
                         f"{fmt(r['hitm_supplied_per_tx'], 0)} | {fmt(r['l2_miss_loads_per_tx'], 0)} | "
                         f"{fmt(r['llc_miss_per_tx'], 1)} | {fmt(r['instructions_per_tx'], 0)} | "
                         f"{fmt(r['cycles_per_tx'], 0)} | {fmt(r['ipc'], 2)} | {fmt(r['effective_ghz'], 2)} |")
    return '\n'.join(lines) + '\n'


CAVEATS = [
    'None durability only; not a durable-commit claim. Close/reopen is not a power-loss test.',
    'Writers: one per physical core, CPUs 0-31 on node 0 then 32-63 on node 1; cells with writers on both nodes have '
    'cross_socket = true. Memory is bound to node 0 in every cell (clients on node 1 access remote memory).',
    'Readers: one per physical core of node 0 (CPUs 8.. for W <= 8, else after the writers), outside every write lock.',
    'A read transaction = begin_read + last key of a random writer already known to have data (every 64th transaction first probes a random writer) + 4 point gets + a scan of 16 keys; all results are '
    'verified exactly (any miss or wrong payload fails the cell). Reader ops = gets + scans.',
    'Latency percentiles are fine-histogram bucket upper bounds (<= 12.5 % wide) over all read transactions of the cell; '
    'writer response percentiles are log2 bucket upper bounds.',
    'Staleness = records committed between a read snapshot and a second begin_read after the transaction (every 16th '
    'transaction), via table.len(); equals committed transactions for all1. redb exposes no public read-transaction id.',
    'clock_ghz is the sampler mean over the WRITER CPUs only (heuristic, whole child process); perf cells report perf '
    'effective_ghz over all threads including readers. clock_off_target (S1/S2) = outside F +- 2 %.',
    'perf counters are process totals: in R > 0 cells the readers\' instructions, cycles and misses are included and '
    'normalised by committed writer transactions.',
    'Three repeats are ranges, not confidence intervals.',
]


def analyze(root, kind):
    identity, rows = load_rows(root, kind)
    analysis = root / f'analysis-{kind}'
    analysis.mkdir(exist_ok=False)
    metrics = {'scale': SCALE_METRICS, 'rw': RW_METRICS, 'perf': PERF_METRICS}[kind]
    table = table_for(rows, metrics)
    clock = base.clock_summary(rows, identity)
    failures = sum(bool(r['failure']) for r in rows)
    base.write_json(analysis / 'summary.json', {
        'cohort_kind': kind, 'table': table, 'rows': rows, 'failures': failures, 'clock': clock,
        'matrix': identity['matrix'][kind], 'caveats': CAVEATS,
        'power': {k: v for k, v in identity['power'].items() if k != 'state'},
        'memory_policy': identity['memory_policy'], 'repeats': identity['repeats'],
        'repeats_cross_socket': identity['repeats_cross_socket']})
    (analysis / 'summary.md').write_text(markdown(kind, table, clock))
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--prepare-only', action='store_true')
    mode.add_argument('--run', choices=KINDS)
    mode.add_argument('--analyze', choices=KINDS)
    parser.add_argument('--output-root', type=Path, required=True)
    parser.add_argument('--build-dir', type=Path, default=OUT)
    parser.add_argument('--numa-node', type=base.parse_numa_node, default=0)
    parser.add_argument('--power-setup', choices=base.POWER_SETUPS)
    parser.add_argument('--fixed-ghz', type=float, default=base.DEFAULT_FIXED_GHZ)
    parser.add_argument('--variants', default=','.join(VARIANTS))
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--repeats-cross-socket', type=int, default=3,
                        help='repeats of the cells with writers on both sockets (trim to 2 only if time requires)')
    args = parser.parse_args()
    root = args.output_root.resolve()
    if args.prepare_only:
        if args.power_setup is None:
            raise RuntimeError('--power-setup is required at prepare')
        variants = [v for v in args.variants.split(',') if v]
        if not variants or any(v not in VARIANTS for v in variants) or len(set(variants)) != len(variants):
            raise RuntimeError(f'--variants must be distinct names from {VARIANTS}')
        if not (1 <= args.repeats_cross_socket <= args.repeats <= len(SEEDS)):
            raise RuntimeError(f'repeats must satisfy 1 <= cross-socket <= repeats <= {len(SEEDS)}')
        prepare(root, args.build_dir.resolve(), args.numa_node, args.power_setup, args.fixed_ghz, variants,
                args.repeats, args.repeats_cross_socket)
    elif args.run:
        run_matrix(root, args.run)
    elif analyze(root, args.analyze):
        raise RuntimeError('analysis contains failed cells; inspect raw results')


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print(f'redb scale/rw campaign: {error}', file=sys.stderr)
        sys.exit(1)
