"""Focused pure topology, CPU ordering and explicit-memory-policy contract tests."""
import argparse
import json
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace

from integration.upscaledb.runner.run_trials import (
    check_memory_selection, cpu_list, discover_topology, parse_cpu_ranges, roles, trial_command)
from integration.upscaledb.experiments.scaling.scaling import (
    artifact_identities, finalize_case, finished_case, select_cpus)


class TopologyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.sysroot = Path(self.temporary.name)
        # Two four-core sockets; each core has two independent logical CPUs.
        for socket in (0, 1):
            node = self.sysroot / 'node' / f'node{socket}'
            node.mkdir(parents=True)
            (node / 'cpulist').write_text(','.join(str(cpu) for core in range(4)
                for cpu in (socket * 8 + core, socket * 8 + core + 16)))
            for core in range(4):
                a, b = socket * 8 + core, socket * 8 + core + 16
                for cpu in (a, b):
                    path = self.sysroot / 'cpu' / f'cpu{cpu}' / 'topology'
                    path.mkdir(parents=True)
                    (path / 'physical_package_id').write_text(str(socket))
                    (path / 'core_id').write_text(str(core))
                    (path / 'thread_siblings_list').write_text(f'{a},{b}')
        self.allowed = [socket * 8 + core + offset for socket in (0, 1)
                        for core in range(4) for offset in (0, 16)]

    def test_core_dedup_excludes_sibling_under_physical_placement(self):
        topology = discover_topology(self.sysroot, self.allowed)
        chosen = select_cpus(topology, 8)
        self.assertEqual(chosen, [0, 8, 1, 9, 2, 10, 3, 11])
        self.assertEqual(len({(topology['cpus'][str(cpu)]['socket'],
                               topology['cpus'][str(cpu)]['core']) for cpu in chosen}), 8)
        self.assertEqual([topology['cpus'][str(cpu)]['socket'] for cpu in chosen[:4]],
                         [0, 1, 0, 1])
        self.assertEqual([topology['cpus'][str(cpu)]['socket'] for cpu in chosen[4:]],
                         [0, 1, 0, 1])

    def test_balanced_socket_order_balances_both_roles(self):
        topology = discover_topology(self.sysroot, self.allowed)
        chosen = select_cpus(topology, 8, 'balanced')
        self.assertEqual(chosen, [0, 8, 1, 9, 2, 10, 3, 11])
        for role in (chosen[:4], chosen[4:]):
            self.assertEqual([sum(topology['cpus'][str(cpu)]['socket'] == socket
                                  for cpu in role) for socket in (0, 1)], [2, 2])

    def test_smt_uses_distinct_logical_siblings_same_physical_cores(self):
        topology = discover_topology(self.sysroot, self.allowed)
        cpus = select_cpus(topology, 4, 'balanced', smt=True)
        self.assertEqual(cpus, [0, 8, 1, 9, 16, 24, 17, 25])
        self.assertEqual(len(cpus), len(set(cpus)))
        self.assertEqual([topology['cpus'][str(cpu)]['core'] for cpu in cpus[:4]],
                         [topology['cpus'][str(cpu)]['core'] for cpu in cpus[4:]])
        with self.assertRaisesRegex(ValueError, 'SMT sibling'):
            select_cpus(discover_topology(self.sysroot, self.allowed[:1]), 1, smt=True)

    def test_inconsistent_sibling_masks_rejected(self):
        (self.sysroot / 'cpu' / 'cpu16' / 'topology' / 'thread_siblings_list').write_text('16')
        with self.assertRaisesRegex(ValueError, 'inconsistent sibling mask'):
            discover_topology(self.sysroot, self.allowed)


class ParserTests(unittest.TestCase):
    def test_linux_ranges_and_rejection(self):
        self.assertEqual(parse_cpu_ranges('0-2,5,8-9'), [0, 1, 2, 5, 8, 9])
        for text in ('', '2-1', '0-2,2-4', '1,,2', '-2', '2-', 'foo'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                parse_cpu_ranges(text)

    def test_worker_and_cpu_bounds(self):
        self.assertEqual(roles('64+64'), (64, 64))
        with self.assertRaises(argparse.ArgumentTypeError):
            roles('65+64')
        with self.assertRaises(argparse.ArgumentTypeError):
            cpu_list('5,5')

    def test_memory_policy_never_falls_back(self):
        self.assertEqual(check_memory_selection('interleave', [1, 0], [0, 1]), [0, 1])
        for policy, nodes in (('bind', []), ('interleave', []), ('first-touch', [0]),
                              (None, [0]), ('bind', [2]), ('unknown', [])):
            with self.subTest(policy=policy, nodes=nodes), self.assertRaises(ValueError):
                check_memory_selection(policy, nodes, [0, 1])


class RunnerTests(unittest.TestCase):
    def test_trial_command_executes_hashed_binary_directly(self):
        options = SimpleNamespace(mode='fixed', cpus=[0, 16], init_cpu=0, init_node=0,
                                  roles=(1, 1), preload=100, reads=400000, inserts=400000,
                                  max_inserts=200000000, memory_limit_gib=8, seed=1,
                                  wait_proxy_ns=1000, seconds=10, warmup=0.5,
                                  memory_policy='bind', memory_nodes=[0])
        command = trial_command(Path('/tmp/upscaledb-native'), options)
        self.assertEqual(command[0], '/tmp/upscaledb-native')
        self.assertEqual(command[command.index('--cpus') + 1], '0,16')
        self.assertEqual(command[command.index('--memory-policy') + 1], 'bind')
        self.assertEqual(command[command.index('--memory-nodes') + 1], '0')

    def test_frozen_binary_and_manifest_hashes_detect_rebuild(self):
        with tempfile.TemporaryDirectory() as root:
            binary_root = Path(root)
            (binary_root / 'upscaledb-native').write_bytes(b'original executable')
            (binary_root / 'build-native.json').write_bytes(b'original manifest')
            frozen = artifact_identities(binary_root, ['native'])
            (binary_root / 'upscaledb-native').write_bytes(b'rebuilt executable')
            self.assertNotEqual(frozen, artifact_identities(binary_root, ['native']))
            (binary_root / 'upscaledb-native').write_bytes(b'original executable')
            (binary_root / 'build-native.json').write_bytes(b'rebuilt manifest')
            self.assertNotEqual(frozen, artifact_identities(binary_root, ['native']))

    def test_resume_only_complete_unchanged_raw_case(self):
        with tempfile.TemporaryDirectory() as path:
            directory = Path(path)
            case = {'name': 'fixed-physical-compact-w1'}
            command = ['python3', '-m', 'integration.upscaledb.runner.run_trials',
                       '--cases', case['name']]
            name = 'block-0000.native.json'
            (directory / 'manifest.json').write_text(json.dumps({
                'config': {'repetitions': 1, 'variants': ['native']},
                'trial_files': [name], 'commands': {name: ['/tmp/upscaledb-native']}}))
            (directory / name).write_text(json.dumps({
                'success': True, 'command': ['/tmp/upscaledb-native']}))
            for source in ('integration-sources.tar.gz', 'source-snapshot.tar.gz'):
                (directory / source).write_bytes(b'archived source')
            finalize_case(directory, case, command)
            finished_case(directory, case, command)
            (directory / name).write_text('{}')
            with self.assertRaisesRegex(ValueError, 'changed or missing'):
                finished_case(directory, case, command)


if __name__ == '__main__':
    unittest.main()
