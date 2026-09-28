#!/usr/bin/env python3
"""Real-database correctness gate for every redb write variant.

Every case is a fresh process on a fresh database file, using the binaries whose
identity build.json records. Per variant (primary binary):
  contents        exact contents after every request, both durabilities, close/reopen;
                  fc_pq: a lone requester takes the E0(b) fast path on all 90 requests
  errors          shape rejections, duplicate-key abort, abort releases the lock,
                  transaction-ID order; bridge variants also: one gate per DB, public
                  begin_write waits for the gate, ID order across gate entry/exit
  savepoints      persistent + ephemeral savepoints across the write phase, restore
  stress-*        16 writers + 2 readers, Immediate and None separately: exact contents,
                  contiguous per-writer-increasing IDs, reads completed during writes,
                  close/reopen; every body charged nonzero service time to its
                  requester and the charged total <= wall time (bodies serialised)
Patched variants additionally run the test_hooks binary:
  stress-*        plus in-body occupancy assertion (max 1), service conservation
                  (charged to requesters == measured on executors) and combiner
                  evidence (FC/FC-PQ ran bodies on other threads; refactored, Mutex,
                  MCS and U-SCL never did)
  paused-writer   reads proceed and see the committed snapshot while a writer is
                  paused inside the lock; a second writer stays blocked
  injected-error  error after inserts: aborted, unchanged, lock released
  panic           (bridge variants) panic inside the body aborts the process
"""
import argparse
import json
from pathlib import Path
import signal

from integration.redb.build import OUT, load_build
from integration.upscaledb.runner.process_execution import capture_command

ROOT = Path(__file__).resolve().parents[2]
VARIANTS = ('native', 'refactored', 'bridge_mutex', 'mcs', 'uscl', 'fc', 'fc_pq')
PRIMARY_CASES = (('contents', ()), ('errors', ()), ('savepoints', ()),
                 ('stress', ('--durability', 'immediate')), ('stress', ('--durability', 'none')))
HOOK_CASES = (('stress', ('--durability', 'immediate')), ('stress', ('--durability', 'none')),
              ('paused-writer', ()), ('injected-error', ()))
TIMEOUT_SECONDS = 180


def case_name(case, extra):
    return case + (('-' + extra[1]) if extra else '')


def run_case(binary, binary_name, variant, case, extra, directory, failstop=False):
    name = case_name(case, extra)
    cell = directory / f'{variant}-{binary_name}-{name}'
    cell.mkdir(parents=True)
    database = cell / 'db.redb'
    command = [str(binary), '--self-test', case, '--variant', variant, '--database', str(database), *extra]
    record = capture_command(command, TIMEOUT_SECONDS, cwd=ROOT,
                             metadata={'variant': variant, 'binary': binary_name, 'case': name})
    if failstop:
        ok = record['returncode'] == -signal.SIGABRT and not record['timeout']
    else:
        ok = record['returncode'] == 0 and not record['timeout']
        if ok:
            try:
                payload = json.loads(record['stdout'])
                ok = (payload.get('status'), payload.get('self_test'), payload.get('variant')) == (
                    'ok', case, variant)
                record['result'] = payload
            except ValueError as error:
                ok, record['parse_error'] = False, str(error)
    record['passed'] = ok
    (cell / 'record.json').write_text(json.dumps(record, indent=2) + '\n')
    if ok and database.exists():
        database.unlink()  # failures keep their database for inspection
    return ok, record


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--build-dir', type=Path, default=OUT)
    parser.add_argument('--output-dir', type=Path, required=True, help='fresh directory for raw case records')
    parser.add_argument('--variants', nargs='+', choices=VARIANTS, default=list(VARIANTS))
    args = parser.parse_args()
    build_dir = args.build_dir.resolve()
    manifest = load_build(build_dir)
    if not manifest['instrumented']:
        raise SystemExit('the gate checks service-time charging: use an instrumented build')
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binaries = {name: build_dir / entry['file'] for name, entry in manifest['binaries'].items()}
    results, failures = [], []
    for variant in args.variants:
        plan = [(manifest['variant_binary'][variant], case, extra, False) for case, extra in PRIMARY_CASES]
        if variant != 'native':
            plan += [('test_hooks', case, extra, False) for case, extra in HOOK_CASES]
            if variant != 'refactored':
                plan.append(('test_hooks', 'panic', (), True))
        for binary_name, case, extra, failstop in plan:
            ok, record = run_case(binaries[binary_name], binary_name, variant, case, extra, output, failstop)
            summary = {'variant': variant, 'binary': binary_name, 'case': case_name(case, extra),
                       'passed': ok, 'details': record.get('result', {}).get('details'),
                       'returncode': record['returncode'], 'timeout': record['timeout']}
            print(json.dumps(summary), flush=True)
            results.append(summary)
            if not ok:
                failures.append(summary)
    report = {'status': 'ok' if not failures else 'failed', 'variants': args.variants,
              'binaries': {k: v['sha256'] for k, v in manifest['binaries'].items()},
              'features': {k: v['resolved_features'] for k, v in manifest['binaries'].items()},
              'redb_source': manifest['redb_source'], 'cases': results, 'failures': failures}
    (output / 'correctness.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'status': report['status'], 'cases': len(results), 'failures': len(failures),
                      'report': str(output / 'correctness.json')}))
    if failures:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
