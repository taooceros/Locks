#!/usr/bin/env python3
"""Record and verify resource partitions for concurrent exploratory probes."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess

from integration.upscaledb._paths import ROOT
from integration.upscaledb.runner.run_trials import discover_topology

PARTITIONS = {
    'H1': {'cpus': list(range(8)), 'memory_node': 0, 'kind': 'DB role/service asymmetry'},
    'H2': {'cpus': [16, 17], 'memory_node': 0, 'kind': 'synthetic reserved-slice diagnostic'},
    'H3': {'cpus': list(range(32, 40)), 'memory_node': 1, 'kind': 'DB intermittent arrivals'},
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-root', type=Path, required=True)
    args = parser.parse_args()
    topology = discover_topology()
    used = set()
    partitions = {}
    for name, spec in PARTITIONS.items():
        identities = []
        siblings = set()
        for cpu in spec['cpus']:
            info = topology['cpus'].get(str(cpu))
            if info is None or info['node'] != spec['memory_node']:
                raise ValueError(f'{name}: unavailable CPU or unexpected NUMA mapping: {cpu}')
            identity = (info['socket'], info['core'])
            if identity in used:
                raise ValueError(f'{name}: physical core overlap: {identity}')
            used.add(identity)
            identities.append(identity)
            siblings.update(info['siblings'])
        partitions[name] = {**spec, 'physical_cores': identities,
                            'excluded_siblings': sorted(siblings - set(spec['cpus']))}
    report = {
        'schema': 1,
        'classification': 'partitioned_concurrent_exploration_not_host_isolation',
        'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'git_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'generator_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'parent_affinity': sorted(os.sched_getaffinity(0)),
        'topology': topology, 'partitions': partitions,
        'notes': [
            'H1 and H3 use different sockets and bind allocation to their local NUMA nodes.',
            'H2 shares socket0 LLC/power/memory resources with H1 despite disjoint physical cores.',
            'All probes still share host/OS resources; compare variants within each hypothesis.',
            'Do not compare raw levels between H1 and H3 as matched placement estimates.',
            'Agents prepare disjoint sources concurrently; all builds/smokes finish before timed co-runs.',
            'No selected SMT siblings are assigned to another probe; this does not reserve CPUs from the OS.',
            'Decisive contrasts require separate serial confirmation; no performance outcome is presumed.',
        ],
    }
    args.output_root.mkdir(parents=True, exist_ok=False)
    (args.output_root / 'cohort.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'output_root': str(args.output_root), 'partitions': partitions}, indent=2))


if __name__ == '__main__':
    main()
