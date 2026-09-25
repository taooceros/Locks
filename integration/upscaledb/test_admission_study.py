"""Observable matched-condition, lifecycle, and frozen-input contract checks."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from types import SimpleNamespace

from admission_study import (ADMISSIONS, METRICS, archive_files, assert_frozen,
                             planned_manifest, record_valid, summarize_trials)
from run_trials import digest


class PlannedMatrixTests(unittest.TestCase):
    def test_five_blocks_cover_each_lifecycle_on_the_same_binary_in_all_placements(self):
        cpus = {}
        cores = {}
        for socket in (0, 1):
            for core in range(8):
                first = socket * 8 + core
                siblings = [first, first + 32]
                cores[f'{socket}:{core}'] = siblings
                for cpu in siblings:
                    cpus[str(cpu)] = {'socket': socket, 'node': socket, 'core': core}
        options = SimpleNamespace(binary_root=Path('/tmp/build'), output_root=Path('/tmp/study'),
                                  backends=('bridge_mutex', 'fc', 'fc_pq', 'uscl', 'cfl_local'),
                                  repetitions=5, reads=400000, inserts=400000, preload=100000,
                                  smoke=False, timeout_seconds=900, order_seed=20260924)
        manifest = planned_manifest(options, {'cores': cores, 'cpus': cpus,
                                              'allowed_cpus': sorted(map(int, cpus))})
        self.assertEqual(len(manifest['schedule']), 200)
        self.assertEqual({case['name'] for case in manifest['placements']},
                         {'single', 'same_socket', 'cross_socket', 'smt'})
        cross = next(case for case in manifest['placements'] if case['name'] == 'cross_socket')
        for role in (cross['cpus'][:4], cross['cpus'][4:]):
            self.assertEqual([sum(cpus[str(cpu)]['socket'] == socket for cpu in role)
                              for socket in (0, 1)], [2, 2])
        for block in range(5):
            current = [item for item in manifest['schedule'] if item['block'] == block]
            self.assertEqual({(item['placement'], item['variant'], item['admission'])
                              for item in current},
                             {(case['name'], backend, admission)
                              for case in manifest['placements'] for backend in options.backends
                              for admission in ADMISSIONS})
            for item in current:
                command = manifest['commands'][item['file']]
                self.assertEqual(command[0], str(options.binary_root / ('upscaledb-' + item['variant'])))
                self.assertEqual(command[-2:], ['--admission', item['admission']])
                self.assertEqual(command[command.index('--warmup') + 1], '0.0')


class MatchedComparisonTests(unittest.TestCase):
    def setUp(self):
        self.cases = [{'name': 'single'}]
        self.backend = ['bridge_mutex']
        self.schedule = [{'block': block, 'placement': 'single', 'variant': 'bridge_mutex',
                          'admission': lifecycle} for block in range(2) for lifecycle in ADMISSIONS]
        self.trials = [dict(item, metrics={'elapsed_s': 4 if item['admission'] == 'counted' else 2,
                                          'total_ops_s': 2 if item['admission'] == 'counted' else 4,
                                          'find_latency_mean_ns': 10 if item['admission'] == 'counted' else 5,
                                          'insert_latency_mean_ns': 12 if item['admission'] == 'counted' else 6,
                                          'timed_process_cpu_s': 8 if item['admission'] == 'counted' else 4})
                       for item in self.schedule]

    def test_same_backend_and_block_isolates_lifecycle_with_consistent_ratio_direction(self):
        rows, groups = summarize_trials(self.trials, self.schedule, self.backend, self.cases, 2)
        self.assertEqual(len(rows), 2)
        self.assertEqual(groups[0]['n_pairs'], 2)
        for metric in METRICS:
            self.assertEqual(groups[0]['speedup_external'][metric]['ratios'], [2, 2])
            self.assertEqual(groups[0]['speedup_external'][metric]['min'], 2)
            self.assertEqual(groups[0]['speedup_external'][metric]['max'], 2)

    def test_rejects_missing_extra_duplicate_or_failed_condition(self):
        scenarios = (self.trials[:-1], self.trials + [self.trials[0]],
                     self.trials[:-1] + [dict(self.trials[-1], variant='fc')],
                     self.trials[:-1] + [dict(self.trials[-1], metrics={**self.trials[-1]['metrics'],
                                                                       'timed_process_cpu_s': 0})])
        for bad in scenarios:
            with self.subTest(bad=bad[-1]), self.assertRaises(ValueError):
                summarize_trials(bad, self.schedule, self.backend, self.cases, 2)


class AdmissionLabelTests(unittest.TestCase):
    def test_rejects_mismatched_record_command_or_harness_label_before_metrics(self):
        item = {'block': 0, 'position': 0, 'placement': 'single', 'variant': 'bridge_mutex',
                'admission': 'external', 'file': 'block-0000.position-00.json'}
        command = ['/tmp/binary', '--mode', 'fixed', '--admission', 'external']
        artifact = {'sha256': 'sha', 'path': command[0]}
        manifest = {'configs': {'single': {'mode': 'fixed'}}, 'commands': {item['file']: command},
                    'artifacts': {'binaries': {'bridge_mutex': artifact}}, 'machine': {}}
        record = {'schema': 1, 'item': item, 'variant': 'bridge_mutex', 'block': 0,
                  'config': manifest['configs']['single'], 'artifacts': manifest['artifacts'],
                  'machine': manifest['machine'], 'command': command,
                  'binary_sha256_before': 'sha', 'binary_sha256_after': 'sha',
                  'admission': 'external', 'result': {'bridge_admission': 'external'}}
        with patch('admission_study.validate', return_value={'flags': {}, 'metrics': {'timed_process_cpu_s': 1}}):
            self.assertEqual(record_valid(record, item, manifest, None)['metrics']['timed_process_cpu_s'], 1)
            for changed in ({'admission': 'counted'}, {'variant': 'fc'}, {'block': 1},
                            {'result': {'bridge_admission': 'counted'}},
                            {'command': command[:-1] + ['counted']}, {'freeze_error': 'changed'}):
                with self.subTest(changed=changed), self.assertRaises(ValueError):
                    record_valid(record | changed, item, manifest, None)
        with self.assertRaisesRegex(ValueError, 'process failed'):
            record_valid(record | {'success': False}, item, manifest, None)


class FrozenInputTests(unittest.TestCase):
    def test_archive_and_pretrial_check_detect_mutated_binary_manifest_library_archive_and_source(self):
        with tempfile.TemporaryDirectory() as location:
            root = Path(location)
            files = {}
            for name in ('binary', 'manifest', 'library', 'rust', 'source'):
                path = root / name
                path.write_bytes(name.encode())
                files[str(path)] = digest(path)
            artifact = {'path': str(root / 'binary'), 'sha256': files[str(root / 'binary')],
                        'build_manifest_path': str(root / 'manifest'),
                        'build_manifest_sha256': files[str(root / 'manifest')]}
            frozen = {'source_files': {str(root / 'source'): files[str(root / 'source')]},
                      'frozen_files': {p: h for p, h in files.items() if p != str(root / 'source')},
                      'artifacts': {'binaries': {'bridge_mutex': artifact}}}
            output = root / 'output'
            output.mkdir()
            saved = archive_files(output, files)
            self.assertEqual(set(saved), set(files))
            with patch('admission_study.build_provenance', return_value=artifact):
                assert_frozen(frozen)
                for name in ('binary', 'manifest', 'library', 'rust', 'source'):
                    path = root / name
                    original = path.read_bytes()
                    path.write_bytes(b'changed')
                    with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'changed'):
                        assert_frozen(frozen)
                    path.write_bytes(original)
                assert_frozen(frozen)


if __name__ == '__main__':
    unittest.main()
