#!/usr/bin/env python3
"""Run fresh-process UpScaleDB trials; no binaries are built by this script.

Examples (CPU IDs/layout are illustrative: supply verified eligible topology;
worker-to-CPU assignment uses the exact ordered CPU list you provide):
  python3 -m integration.upscaledb.runner.run_trials --variants native,profile --cpus 0,1,2,3 --layout packed
  python3 -m integration.upscaledb.runner.run_trials --variants native --mode fixed --roles 1 --cpus 0 --layout packed --repetitions 1 --preload 1000 --reads 100 --inserts 100 --warmup 0 --smoke
  python3 -m integration.upscaledb.runner.run_trials --variants native,refactored,bridge_mutex,fc,fc_pq --cpus 0,1,2,3 --layout packed

Build manifests (build-VARIANT.json beside the default binaries) are mandatory;
use --build-manifest VARIANT=PATH with --binary for external builds.
Default: 10 blocks, 4+4 workers / 4 explicitly provided CPUs / 120-second duration.
--smoke labels a deliberately small exploratory run; it never changes counts implicitly.
--layout records an explicitly chosen CPU placement label, not an inferred topology.
"""
import argparse
import base64
import datetime as dt
import hashlib
import io
import json
import os
from pathlib import Path
import math
import platform
import random
import resource
import signal
import subprocess
import sys
import tarfile
import tempfile
import time

from integration.upscaledb._paths import CORE, EXPERIMENTS, REPORTS, ROOT, TESTS, UPSCALEDB

HERE = Path(__file__).resolve().parent
DEFAULT_OUTPUT = ROOT / '.worktree' / 'upscaledb' / 'output'
VARIANTS = ('native', 'refactored', 'bridge_mutex', 'fc', 'fc_pq', 'uscl', 'cfl_local',
            'spinlock', 'mcs', 'ticket', 'clh', 'profile',
            'bridge_mutex_profile', 'fc_profile', 'fc_pq_profile',
            'uscl_profile', 'cfl_local_profile', 'spinlock_profile', 'mcs_profile',
            'ticket_profile', 'clh_profile')


def digest(path):
    h = hashlib.sha256()
    try:
        with path.open('rb') as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b''):
                h.update(block)
    except OSError:
        return None
    return h.hexdigest()

def capture_sources(outdir, variants):
    """Archive the exact integration inputs, including Rust bridge and lock source."""
    paths = [CORE / name for name in ('bridge.h', 'private_ops.h', 'native-lock-timing.patch',
                                      'bridge-ops.patch', 'native_harness.cc', 'build.py')]
    paths.extend((HERE / 'run_trials.py', REPORTS / 'analyze.py', UPSCALEDB / '_paths.py',
                  EXPERIMENTS / 'scaling/scaling.py', TESTS / 'test_scaling.py'))
    rust = ROOT / 'crates' / 'upscaledb-bridge'
    if any(v not in ('native', 'refactored', 'profile') for v in variants):
        if not (rust / 'src/lib.rs').is_file():
            raise ValueError('Rust bridge source is missing; cannot establish bridge provenance')
        from integration.upscaledb.core.build import rust_sources_files
        paths.extend(rust_sources_files())
    if rust.is_dir():
        paths.extend(p for p in rust.rglob('*') if p.is_file() and not p.is_symlink()
                     and 'target' not in p.parts)
    paths.extend([ROOT / 'Cargo.toml', ROOT / 'Cargo.lock',
                  ROOT / 'crates/libdlock/src/dlock2/fc/lock.rs',
                  ROOT / 'crates/libdlock/src/dlock2/fc_pq/lock.rs'])
    if any(v.startswith('uscl') for v in variants):
        paths.extend((ROOT / 'c/u-scl' / name for name in
                     ('fairlock.h', 'fairlock.c', 'common.h', 'rdtsc.h')))
        paths.extend((ROOT / 'crates/libdlock/src' / name for name in
                     ('u_scl.rs', 'dlock2/uscl.rs')))
    if any(v.startswith('cfl_local') for v in variants):
        paths.extend((ROOT / 'c/cfl' / name for name in ('cfl.c', 'cfl.h')))
        paths.extend((ROOT / 'crates/libdlock/src/dlock2' / name for name in
                     ('cfl.rs', 'c_cfl.rs')))
    inventory = {}
    target = outdir / 'integration-sources.tar.gz'
    fd, temporary = tempfile.mkstemp(prefix='.integration-sources.', dir=outdir)
    os.close(fd)
    try:
        with tarfile.open(temporary, 'w:gz') as archive:
            for path in sorted(set(paths)):
                if not path.is_file() or path.is_symlink():
                    raise ValueError('missing or unsafe provenance input: ' + str(path))
                data = path.read_bytes()
                name = str(path.relative_to(ROOT))
                info = tarfile.TarInfo(name)
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
                inventory[name] = hashlib.sha256(data).hexdigest()
        os.link(temporary, target)
    finally:
        os.unlink(temporary)
    return {'file': target.name, 'sha256': digest(target), 'files': inventory}


def build_provenance(variant, binary, path):
    if not path.is_file():
        raise ValueError(f'{variant}: missing authoritative build manifest: {path}')
    try:
        manifest = json.loads(path.read_text())
    except (OSError, ValueError) as exc:
        raise ValueError(f'{variant}: unreadable build manifest {path}: {exc}') from exc
    actual = digest(binary)
    if (not isinstance(manifest, dict) or manifest.get('schema') != 2 or
            manifest.get('variant') != variant or
            manifest.get('hashes', {}).get('binary_sha256') != actual or
            Path(manifest.get('binary', '')).resolve() != binary.resolve()):
        raise ValueError(f'{variant}: build manifest does not identify executable {binary}')
    if manifest.get('hashes', {}).get('harness_sha256') != digest(CORE / 'native_harness.cc'):
        raise ValueError(f'{variant}: build manifest harness hash differs from current harness; rebuild')
    if manifest.get('hashes', {}).get('build_py_sha256') != digest(CORE / 'build.py'):
        raise ValueError(f'{variant}: build recipe differs from manifest; rebuild with current recipe')
    source = Path(manifest.get('source', ''))
    recorded = manifest.get('source_verification', {})
    if not source.is_dir() or not recorded.get('head') or not recorded.get('implementation_sha256'):
        raise ValueError(f'{variant}: build manifest lacks verified source checkout')
    if recorded.get('profile_patch_sha256') is not None and (
            recorded['profile_patch_sha256'] != digest(CORE / 'native-lock-timing.patch')):
        raise ValueError(f'{variant}: native timing patch differs from build manifest')
    if recorded.get('bridge_patch_sha256') is not None and (
            recorded['bridge_patch_sha256'] != digest(CORE / 'bridge-ops.patch')):
        raise ValueError(f'{variant}: bridge patch differs from build manifest')
    for name, filename in (('bridge_h_sha256', 'bridge.h'),
                           ('private_ops_sha256', 'private_ops.h')):
        if manifest.get('hashes', {}).get(name) != digest(CORE / filename):
            raise ValueError(f'{variant}: {filename} differs from build manifest')
    implementation = source / 'src/5upscaledb/upscaledb.cc'
    if digest(implementation) != recorded['implementation_sha256']:
        raise ValueError(f'{variant}: source implementation differs from build manifest')
    lib = manifest.get('library_verification', {})
    if lib.get('shared_object_sha256') != digest(Path(lib.get('shared_object', ''))):
        raise ValueError(f'{variant}: library differs from build manifest')
    reuse = manifest.get('database_reuse')
    if reuse is not None:
        from integration.upscaledb.core.build import database_reuse_provenance
        previous = Path(reuse.get('build_manifest', ''))
        toolchain = manifest.get('toolchain', {})
        if not previous.is_file() or digest(previous) != reuse.get('build_manifest_sha256'):
            raise ValueError(f'{variant}: frozen database manifest changed')
        verified, _ = database_reuse_provenance(
            previous.parent, manifest['source_kind'], Path(toolchain['boost_store']),
            Path(toolchain['boost_dev_store']), Path(toolchain['gcc_store']))
        if (verified != reuse or reuse.get('source_verification') != recorded or
                reuse.get('library_verification') != lib or reuse.get('toolchain') != toolchain):
            raise ValueError(f'{variant}: frozen database provenance chain differs from build')
    rust = manifest.get('rust_staticlib')
    if rust is not None:
        from integration.upscaledb.core.build import rust_sources_digest
        if (rust.get('archive_sha256') != digest(Path(rust.get('archive', ''))) or
                rust.get('rust_sources_sha256') != rust_sources_digest()):
            raise ValueError(f'{variant}: Rust archive or its source inputs differ from build')
    return {'path': str(binary), 'sha256': actual, 'build_manifest_path': str(path),
            'build_manifest_sha256': digest(path), 'build_manifest': manifest,
            'source': git_info(source), 'source_implementation_sha256': digest(implementation),
            'patched_library_sha256': lib['shared_object_sha256']}


def command_output(command, cwd=ROOT):
    try:
        result = subprocess.run(command, cwd=cwd, capture_output=True, text=True,
                                errors='replace', timeout=8, check=False)
        return result.stdout.strip() if result.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def git_info(path):
    if not path.is_dir():
        return None
    head = command_output(['git', 'rev-parse', 'HEAD'], path)
    if head is None:
        return None
    # Hash the actual tracked diff and untracked file contents, not merely a dirty flag.
    try:
        diff = subprocess.run(['git', 'diff', '--binary', 'HEAD', '--'], cwd=path,
                              capture_output=True, timeout=15, check=True).stdout
        names = subprocess.run(['git', 'ls-files', '--others', '--exclude-standard', '-z'],
                               cwd=path, capture_output=True, timeout=15, check=True).stdout
        h = hashlib.sha256(diff)
        untracked = []
        for raw in sorted(filter(None, names.split(b'\0'))):
            name = os.fsdecode(raw)
            sha = digest(path / name)
            untracked.append({'path': name, 'sha256': sha})
            h.update(raw + b'\0' + (sha or 'unreadable').encode() + b'\0')
        return {'head': head, 'dirty': bool(diff or names), 'diff_sha256': h.hexdigest(),
                'tracked_diff_sha256': hashlib.sha256(diff).hexdigest(),
                'untracked': untracked}
    except (OSError, subprocess.SubprocessError):
        return {'head': head, 'dirty': None, 'diff_sha256': None, 'error': 'diff unavailable'}


def read_text(path):
    try:
        return Path(path).read_text(errors='replace').strip()
    except OSError:
        return None


def source_snapshot(outdir):
    """Archive only a bounded root HEAD diff and untracked regular files."""
    try:
        patch = subprocess.run(['git', 'diff', '--binary', 'HEAD', '--'], cwd=ROOT,
                               capture_output=True, timeout=20, check=True).stdout
        names = subprocess.run(['git', 'ls-files', '--others', '--exclude-standard', '-z'],
                               cwd=ROOT, capture_output=True, timeout=20, check=True).stdout
    except (OSError, subprocess.SubprocessError) as exc:
        return {'error': 'git snapshot unavailable: ' + str(exc)}
    limit = 32 * 1024 * 1024
    if len(patch) > limit:
        return {'error': 'tracked diff exceeds 32 MiB snapshot ceiling',
                'tracked_diff_sha256': hashlib.sha256(patch).hexdigest()}
    target = outdir / 'source-snapshot.tar.gz'
    fd, temporary = tempfile.mkstemp(prefix='.source-snapshot.', dir=outdir)
    os.close(fd)
    skipped = []
    remaining = limit - len(patch)
    try:
        with tarfile.open(temporary, 'w:gz') as archive:
            info = tarfile.TarInfo('tracked.diff')
            info.size = len(patch)
            archive.addfile(info, io.BytesIO(patch))
            for raw in sorted(filter(None, names.split(b'\0'))):
                name = os.fsdecode(raw)
                path = Path(name)
                source = ROOT / path
                if (path.is_absolute() or '..' in path.parts or
                        path.parts[0] in ('.worktree', '.git') or
                        source.is_symlink() or not source.is_file()):
                    skipped.append({'path': name, 'reason': 'unsafe or not a regular source file'})
                    continue
                try:
                    size = source.stat().st_size
                    if size > 2 * 1024 * 1024 or size > remaining:
                        skipped.append({'path': name, 'reason': 'snapshot size limit'})
                        continue
                    content = source.read_bytes()
                except OSError:
                    skipped.append({'path': name, 'reason': 'source changed during snapshot'})
                    continue
                if len(content) > 2 * 1024 * 1024 or len(content) > remaining:
                    skipped.append({'path': name, 'reason': 'snapshot size limit'})
                    continue
                info = tarfile.TarInfo('untracked/' + name)
                info.size = len(content)
                archive.addfile(info, io.BytesIO(content))
                remaining -= len(content)
        os.link(temporary, target)
    finally:
        os.unlink(temporary)
    return {'file': target.name, 'sha256': digest(target),
            'tracked_diff_sha256': hashlib.sha256(patch).hexdigest(),
            'skipped_untracked': skipped}


def parse_cpu_ranges(text):
    """Parse Linux cpulists, rejecting overlapping IDs and malformed ranges."""
    if not isinstance(text, str) or not text.strip():
        raise ValueError('empty Linux CPU/node list')
    ids = set()
    for part in text.strip().split(','):
        bounds = part.split('-')
        if len(bounds) not in (1, 2) or not all(x.isdecimal() for x in bounds):
            raise ValueError('malformed Linux CPU/node list: ' + text)
        start = int(bounds[0])
        end = int(bounds[-1])
        if end < start or any(n in ids for n in range(start, end + 1)):
            raise ValueError('overlapping/reversed Linux CPU/node list: ' + text)
        ids.update(range(start, end + 1))
    return sorted(ids)


def discover_topology(sys_root=Path('/sys/devices/system'), allowed=None):
    """Record sysfs socket/core/NUMA/sibling facts for every eligible logical CPU."""
    allowed = sorted(os.sched_getaffinity(0) if allowed is None else allowed)
    if not allowed or len(allowed) != len(set(allowed)):
        raise ValueError('affinity must contain distinct eligible CPUs')
    nodes = {}
    for path in sorted((sys_root / 'node').glob('node[0-9]*')):
        node = int(path.name[4:])
        nodes[node] = parse_cpu_ranges((path / 'cpulist').read_text())
    if not nodes:
        raise ValueError('NUMA node topology unavailable in sysfs')
    cpus = {}
    for cpu in allowed:
        directory = sys_root / 'cpu' / f'cpu{cpu}' / 'topology'
        try:
            socket = int((directory / 'physical_package_id').read_text().strip())
            core = int((directory / 'core_id').read_text().strip())
            siblings = parse_cpu_ranges((directory / 'thread_siblings_list').read_text())
        except (OSError, ValueError) as exc:
            raise ValueError(f'CPU {cpu}: incomplete sysfs topology: {exc}') from exc
        owning_nodes = [node for node, members in nodes.items() if cpu in members]
        if len(owning_nodes) != 1 or cpu not in siblings:
            raise ValueError(f'CPU {cpu}: missing/ambiguous node or sibling topology')
        cpus[cpu] = {'socket': socket, 'core': core, 'node': owning_nodes[0],
                     'siblings': siblings, 'allowed_siblings': sorted(set(siblings) & set(allowed))}
    cores = {}
    for cpu, item in cpus.items():
        key = f"{item['socket']}:{item['core']}"
        cores.setdefault(key, []).append(cpu)
    for key, members in cores.items():
        if any(set(cpus[cpu]['allowed_siblings']) != set(members) for cpu in members):
            raise ValueError(f'inconsistent sibling mask for core {key}')
    return {'allowed_cpus': allowed, 'nodes': {str(k): v for k, v in nodes.items()},
            'cpus': {str(k): v for k, v in cpus.items()},
            'cores': {k: sorted(v) for k, v in cores.items()}}


def environment(cpus):
    mem = read_text('/proc/meminfo')
    memory = {line.split(':', 1)[0]: line.split(':', 1)[1].strip()
              for line in (mem or '').splitlines() if ':' in line}
    proc_status = read_text('/proc/self/status') or ''
    capabilities = {line.split(':', 1)[0]: line.split(':', 1)[1].strip()
                    for line in proc_status.splitlines() if line.startswith(('CapEff:', 'CapBnd:'))}
    status_fields = {line.split(':', 1)[0]: line.split(':', 1)[1].strip()
                     for line in proc_status.splitlines() if ':' in line}
    cpuinfo = read_text('/proc/cpuinfo') or ''
    processors = cpuinfo.split('\n\n')
    first_cpu = {line.split(':', 1)[0].strip(): line.split(':', 1)[1].strip()
                 for line in processors[0].splitlines() if ':' in line}
    cpu_identity = {key: first_cpu.get(key) for key in
                    ('vendor_id', 'model name', 'cpu family', 'model', 'stepping',
                     'microcode', 'cache size', 'siblings', 'cpu cores', 'flags')}
    topology = {str(cpu): {
        name: read_text(f'/sys/devices/system/cpu/cpu{cpu}/topology/{name}')
        for name in ('physical_package_id', 'core_id', 'thread_siblings_list')}
        for cpu in cpus}
    limits = {}
    for name in ('RLIMIT_AS', 'RLIMIT_CPU', 'RLIMIT_DATA', 'RLIMIT_NOFILE'):
        if hasattr(resource, name):
            limits[name] = list(resource.getrlimit(getattr(resource, name)))
    versions = {name: command_output([name, '--version']) for name in
                ('gcc', 'g++', 'rustc', 'cargo', 'devenv', 'nix', 'python3')}
    return {'timestamp_utc': dt.datetime.now(dt.timezone.utc).isoformat(),
            'platform': platform.platform(), 'uname': list(platform.uname()),
            'python': sys.version, 'cpu': {'online_processor_entries': len(processors),
                                           'identity': cpu_identity, 'selected_topology': topology},
            'affinity': sorted(os.sched_getaffinity(0)) if hasattr(os, 'sched_getaffinity') else None,
            'scheduler': {'policy': os.sched_getscheduler(0),
                          'priority': os.sched_getparam(0).sched_priority,
                          'nice': os.getpriority(os.PRIO_PROCESS, 0),
                          'cpus_allowed_list': status_fields.get('Cpus_allowed_list')},
            'numa': {'mems_allowed_list': status_fields.get('Mems_allowed_list'),
                     'numactl_show': command_output(['numactl', '--show']),
                     'placement_claim': 'actual worker allocations not proven node-bound; first-touch possible'},
            'selected_cpu_governors': {str(cpu): read_text(
                f'/sys/devices/system/cpu/cpu{cpu}/cpufreq/scaling_governor') for cpu in cpus},
            'memory': {k: memory.get(k) for k in ('MemTotal', 'MemAvailable', 'SwapTotal')},
            'perf_event_paranoid': read_text('/proc/sys/kernel/perf_event_paranoid'),
            'kptr_restrict': read_text('/proc/sys/kernel/kptr_restrict'),
            'perf_capabilities': capabilities,
            'kernel_lockdown': read_text('/sys/kernel/security/lockdown'),
            'devenv': {'DEVENV_ROOT': os.environ.get('DEVENV_ROOT'),
                       'IN_NIX_SHELL': os.environ.get('IN_NIX_SHELL'),
                       'NIX_CFLAGS_COMPILE': os.environ.get('NIX_CFLAGS_COMPILE'),
                       'NIX_LDFLAGS': os.environ.get('NIX_LDFLAGS'),
                       'lock_sha256': digest(ROOT / 'devenv.lock'),
                       'yaml_sha256': digest(ROOT / 'devenv.yaml')},
            'toolchain_versions': versions, 'resource_limits': limits,
            'git': git_info(ROOT)}


def positive_int(value):
    result = int(value)
    if result < 1:
        raise argparse.ArgumentTypeError('must be positive')
    return result


def roles(value):
    if value == '1':
        return (1, 0)
    try:
        finders, inserters = map(int, value.split('+'))
    except (ValueError, TypeError):
        raise argparse.ArgumentTypeError('roles must be 1 or F+I, totaling 2..128') from None
    if finders < 1 or inserters < 1 or finders + inserters > 128:
        raise argparse.ArgumentTypeError('roles must total 2..128 with both roles present')
    return finders, inserters


def cpu_list(value):
    try:
        cpus = [int(part) for part in value.split(',')]
    except ValueError:
        raise argparse.ArgumentTypeError('CPUs must be an explicit comma-separated list') from None
    if not cpus or len(cpus) != len(set(cpus)) or any(n < 0 or n >= 1024 for n in cpus):
        raise argparse.ArgumentTypeError('invalid, duplicate or out-of-range CPU')
    return cpus
def check_memory_selection(policy, nodes, allowed_nodes):
    if policy not in (None, 'first-touch', 'bind', 'interleave'):
        raise ValueError('unknown memory policy')
    if policy in ('bind', 'interleave') and not nodes:
        raise ValueError('bind/interleave requires explicit memory nodes')
    if policy not in ('bind', 'interleave') and nodes:
        raise ValueError('first-touch/inherited policy forbids memory nodes')
    if nodes and not set(nodes) <= set(allowed_nodes):
        raise ValueError('requested memory nodes outside allowed NUMA mask')
    return sorted(nodes or [])




def atomic_new(path, obj):
    # Link is atomic and refuses overwrites, including a concurrent runner's output.
    fd, temporary = tempfile.mkstemp(prefix='.' + path.name + '.', dir=path.parent)
    try:
        with os.fdopen(fd, 'w', encoding='utf-8') as stream:
            json.dump(obj, stream, indent=2, sort_keys=True, allow_nan=False)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
        os.link(temporary, path)
    finally:
        os.unlink(temporary)


class TrialStop(Exception):
    def __init__(self, signum):
        self.signum = signum
        super().__init__(signal.Signals(signum).name)


def request_stop(signum, _frame):
    raise TrialStop(signum)


def stop_child_group(child):
    # Ignore repeated terminal signals while cleaning up this verified child.
    previous = {sig: signal.signal(sig, signal.SIG_IGN) for sig in (signal.SIGINT, signal.SIGTERM)}
    cleanup_error = None
    try:
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            out, err = child.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                out, err = child.communicate(timeout=3)
            except subprocess.TimeoutExpired as exc:
                # A detached descendant may still hold an inherited pipe open.
                out, err = exc.stdout or b'', exc.stderr or b''
                child.stdout.close()
                child.stderr.close()
                try:
                    child.kill()
                except ProcessLookupError:
                    pass
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    cleanup_error = 'child did not exit after process-group SIGKILL'
        return out, err, cleanup_error
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def reject_nonfinite(token):
    raise ValueError('nonfinite JSON constant: ' + token)


def run_trial(command, timeout, expected_sha256):
    start = time.monotonic_ns()
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    timed_out = False
    interrupted_signal = None
    out = err = b''
    returncode = None
    launch_error = None
    cleanup_error = None
    actual_before = digest(Path(command[0]))
    artifact_error = None if actual_before == expected_sha256 else 'binary SHA-256 changed before trial'
    child = None
    if artifact_error is None:
        try:
            # Defer signals until Popen has returned and `child` is owned; otherwise
            # a signal between fork and assignment could leave an orphaned group.
            old_mask = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGINT, signal.SIGTERM})
            try:
                child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                         start_new_session=True, cwd=ROOT)
            finally:
                signal.pthread_sigmask(signal.SIG_SETMASK, old_mask)
            try:
                out, err = child.communicate(timeout=timeout)
            except subprocess.TimeoutExpired:
                timed_out = True
                out, err, cleanup_error = stop_child_group(child)
            returncode = child.returncode
        except TrialStop as stop:
            interrupted_signal = stop.signum
            if child is not None:
                out, err, cleanup_error = stop_child_group(child)
                returncode = child.returncode
        except OSError as ex:
            launch_error = str(ex)
    actual_after = digest(Path(command[0]))
    if actual_after != expected_sha256:
        artifact_error = 'binary SHA-256 changed during trial'
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu_ns = round(((after.ru_utime - before.ru_utime) +
                    (after.ru_stime - before.ru_stime)) * 1e9)
    stdout = out.decode('utf-8', errors='replace')
    stderr = err.decode('utf-8', errors='replace')
    try:
        parsed = json.loads(stdout, parse_constant=reject_nonfinite)
        if not isinstance(parsed, dict):
            parsed = None
    except (ValueError, TypeError):
        parsed = None
    return {'command': command, 'returncode': returncode, 'stdout': stdout, 'stderr': stderr,
            'stdout_base64': base64.b64encode(out).decode('ascii'),
            'stderr_base64': base64.b64encode(err).decode('ascii'),
            'elapsed_wall_ns': time.monotonic_ns() - start,
            'process_cpu_time_ns': cpu_ns, 'timed_out': timed_out,
            'interrupted_signal': interrupted_signal, 'binary_sha256_before': actual_before,
            'binary_sha256_after': actual_after, 'artifact_error': artifact_error,
            'launch_error': launch_error, 'result': parsed,
            'cleanup_error': cleanup_error,
            'success': not timed_out and interrupted_signal is None and not artifact_error and
                       not launch_error and not cleanup_error and returncode == 0 and
                       parsed is not None and parsed.get('status') == 'ok'}


def trial_command(binary, args):
    command = [str(binary), '--mode', args.mode, '--cpus',
               ','.join(map(str, args.cpus)), '--init-cpu', str(args.init_cpu),
               '--init-node', str(args.init_node)]
    command += (['--workers', '1'] if args.roles == (1, 0) else
                ['--finders', str(args.roles[0]), '--inserters', str(args.roles[1])])
    for key, value in (('preload', args.preload), ('reads', args.reads),
                       ('inserts', args.inserts), ('max-inserts', args.max_inserts),
                       ('memory-limit-gib', args.memory_limit_gib), ('seed', args.seed),
                       ('wait-proxy-ns', args.wait_proxy_ns), ('seconds', args.seconds),
                       ('warmup', args.warmup)):
        command += ['--' + key, str(value)]
    if args.memory_policy:
        command += ['--memory-policy', args.memory_policy]
        if args.memory_nodes:
            command += ['--memory-nodes', ','.join(map(str, args.memory_nodes))]
    return command


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--variants', required=True,
                        help='comma-separated built variants (primary or separately labeled profiles)')
    parser.add_argument('--binary', action='append', default=[], metavar='VARIANT=PATH',
                        help='override executable for a named variant; repeat as needed')
    parser.add_argument('--build-manifest', action='append', default=[], metavar='VARIANT=PATH',
                        help='authoritative manifest for each external binary (default build-VARIANT.json)')
    parser.add_argument('--mode', choices=('fixed', 'duration'), default='duration')
    parser.add_argument('--roles', type=roles, default=(4, 4), metavar='F+I',
                        help='1 (alternating) or F+I totaling 2..128; default 4+4')
    parser.add_argument('--cpus', required=True, type=cpu_list,
                        help='eligible CPUs, ordered for round-robin worker assignment; mandatory')
    parser.add_argument('--layout', choices=('packed', 'split'), required=True,
                        help='user-specified CPU placement label; CPU IDs/order must be supplied')
    parser.add_argument('--exclusive-cpus', action='store_true',
                        help='require one distinct logical CPU per worker (scaling/SMT study)')
    parser.add_argument('--stop-on-failure', action='store_true',
                        help='retain failed raw trial then stop; default continues entire matrix')
    parser.add_argument('--init-cpu', type=int, help='explicit initialization CPU (default first selected CPU)')
    parser.add_argument('--init-node', type=int, help='explicit initialization node')
    parser.add_argument('--memory-policy', choices=('first-touch', 'bind', 'interleave'),
                        help='explicit enforced Linux process memory policy; omitted preserves legacy inherited policy')
    parser.add_argument('--memory-nodes', type=cpu_list, help='comma-separated NUMA nodes for bind/interleave')
    parser.add_argument('--repetitions', type=positive_int, default=10)
    parser.add_argument('--seconds', type=float, default=120)
    parser.add_argument('--warmup', type=float, default=5)
    parser.add_argument('--seed', type=int, default=1, help='harness workload seed')
    parser.add_argument('--order-seed', type=int, default=20260923, help='independent order RNG seed')
    parser.add_argument('--reads', type=int, default=400000)
    parser.add_argument('--inserts', type=int, default=400000)
    parser.add_argument('--preload', type=int, default=100000)
    parser.add_argument('--max-inserts', type=int, default=200000000)
    parser.add_argument('--memory-limit-gib', type=int, default=8)
    parser.add_argument('--wait-proxy-ns', type=int, default=1000)
    parser.add_argument('--timeout-seconds', type=float, default=None,
                        help='whole process including setup/warmup/verify; default max(900, seconds+warmup+180)')
    parser.add_argument('--output-dir', type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument('--smoke', action='store_true', help='explicitly label exploratory smoke; no implicit downsizing')
    args = parser.parse_args()
    variants = args.variants.split(',')
    if not variants or len(set(variants)) != len(variants) or any(v not in VARIANTS for v in variants):
        parser.error('variants must be distinct names from ' + ', '.join(VARIANTS))
    if hasattr(os, 'sched_getaffinity') and not set(args.cpus) <= os.sched_getaffinity(0):
        parser.error('selected CPUs must be within the current process affinity mask')
    if args.exclusive_cpus and len(args.cpus) != sum(args.roles):
        parser.error('exclusive placement requires exactly one distinct CPU per worker')
    if args.init_cpu is None:
        args.init_cpu = args.cpus[0]
    if args.init_cpu not in os.sched_getaffinity(0):
        parser.error('initialization CPU is not in allowed affinity mask')
    # No implicit bind/interleave to first-touch downgrade is permitted.
    try:
        topology = discover_topology()
        init_topology = topology['cpus'].get(str(args.init_cpu))
        if init_topology is None:
            parser.error('initialization CPU topology not available')
        if args.init_node is None:
            args.init_node = init_topology['node']
        if args.init_node != init_topology['node']:
            parser.error('initialization CPU is not on initialization node')
        available_mems = parse_cpu_ranges(next(
            line.split(':', 1)[1].strip() for line in
            (read_text('/proc/self/status') or '').splitlines()
            if line.startswith('Mems_allowed_list:')))
        args.memory_nodes = check_memory_selection(args.memory_policy, args.memory_nodes, available_mems)
    except (ValueError, OSError, StopIteration) as exc:
        parser.error('NUMA topology discovery failed: ' + str(exc))
    if not (0.01 <= args.seconds <= 600 and 0 <= args.warmup <= 60 and 0 <= args.seed < 2**64):
        parser.error('seconds, warmup or seed outside harness bounds')
    if not (1 <= args.preload <= 1000000 and 0 <= args.reads <= 2000000 and
            0 <= args.inserts <= args.max_inserts <= 200000000 and
            1 <= args.memory_limit_gib <= 64 and args.wait_proxy_ns >= 0):
        parser.error('workload or resource limit outside harness bounds')
    if args.mode == 'fixed' and (args.reads == 0 or args.inserts == 0):
        parser.error('fixed-work comparisons require positive counts for both operation types')
    if args.smoke and (args.repetitions > 2 or args.warmup > 1 or args.preload > 10000 or
                       (args.mode == 'duration' and args.seconds > 5) or
                       (args.mode == 'fixed' and args.reads + args.inserts > 10000)):
        parser.error('--smoke requires <=2 repetitions, <=1s warmup, <=10000 preload, '
                     '<=5s duration or <=10000 total fixed operations; set small values explicitly')
    timeout = args.timeout_seconds if args.timeout_seconds is not None else max(900, args.seconds + args.warmup + 180)
    if not math.isfinite(timeout) or not 0 < timeout < 86400:
        parser.error('timeout must be finite, positive and less than one day')
    overrides = {}
    for entry in args.binary:
        variant, separator, path = entry.partition('=')
        if not separator or variant not in variants or variant in overrides or not path:
            parser.error('--binary needs one unique VARIANT=PATH per selected variant')
        overrides[variant] = Path(path).expanduser().resolve()
    binaries = {v: overrides.get(v, ROOT / '.worktree' / 'upscaledb' / ('upscaledb-' + v))
                for v in variants}
    for name, binary in binaries.items():
        if not binary.is_file() or not os.access(binary, os.X_OK):
            parser.error(f'{name}: unavailable executable {binary}; build/provide it explicitly')
    manifest_overrides = {}
    for entry in args.build_manifest:
        variant, separator, path = entry.partition('=')
        if not separator or variant not in variants or variant in manifest_overrides or not path:
            parser.error('--build-manifest needs one unique VARIANT=PATH per selected variant')
        manifest_overrides[variant] = Path(path).expanduser().resolve()
    outdir = args.output_dir.expanduser().resolve()
    if outdir.exists() and any(outdir.iterdir()):
        parser.error('refusing to reuse a nonempty output directory; choose a fresh path')
    outdir.mkdir(parents=True, exist_ok=True)
    config = {'variants': variants, 'mode': args.mode, 'finders': args.roles[0],
              'inserters': args.roles[1], 'workers': sum(args.roles), 'cpus': args.cpus,
              'layout': args.layout, 'repetitions': args.repetitions, 'seconds': args.seconds,
              'warmup': args.warmup, 'seed': args.seed, 'order_seed': args.order_seed,
              'reads': args.reads, 'inserts': args.inserts, 'preload': args.preload,
              'max_inserts': args.max_inserts, 'memory_limit_gib': args.memory_limit_gib,
              'wait_proxy_ns': args.wait_proxy_ns, 'timeout_seconds': timeout, 'smoke': args.smoke,
              'exclusive_cpus': args.exclusive_cpus, 'init_cpu': args.init_cpu,
              'stop_on_failure': args.stop_on_failure,
              'init_node': args.init_node, 'requested_memory_policy': args.memory_policy,
              'memory_nodes': args.memory_nodes or []}
    try:
        built = {name: build_provenance(
            name, binary, manifest_overrides.get(
                name, ROOT / '.worktree' / 'upscaledb' / ('build-' + name + '.json')))
            for name, binary in binaries.items()}
        source_archive = capture_sources(outdir, variants)
    except ValueError as exc:
        parser.error(str(exc))
    provenance = {'runner_sha256': digest(Path(__file__)),
                  'build_recipe_sha256': digest(CORE / 'build.py'),
                  'native_harness_sha256': digest(CORE / 'native_harness.cc'),
                  'native_profile_patch_sha256': digest(CORE / 'native-lock-timing.patch'),
                  'bridge_header_sha256': digest(CORE / 'bridge.h'),
                  'integration_sources': source_archive, 'binaries': built}
    if any(details['sha256'] is None for details in built.values()):
        parser.error('binary disappeared before its SHA-256 could be recorded')
    provenance['source_snapshot'] = source_snapshot(outdir)
    if 'error' in provenance['source_snapshot']:
        parser.error(provenance['source_snapshot']['error'])
    machine = environment(args.cpus)
    machine['discovered_topology'] = topology
    machine['numa']['auto_balancing'] = read_text('/proc/sys/kernel/numa_balancing')
    rng = random.Random(args.order_seed)
    orders = []
    for block in range(args.repetitions):
        order = variants.copy()
        rng.shuffle(order)
        orders.append(order)
    trial_files = [f'block-{block:04d}.{variant}.json'
                   for block, order in enumerate(orders) for variant in order]
    commands = {f'block-{block:04d}.{variant}.json': trial_command(binaries[variant], args)
                for block, order in enumerate(orders) for variant in order}
    atomic_new(outdir / 'manifest.json', {'schema': 1, 'config': config, 'machine': machine,
                                           'artifacts': provenance, 'orders': orders,
                                           'trial_files': trial_files, 'commands': commands})
    signal.signal(signal.SIGINT, request_stop)
    signal.signal(signal.SIGTERM, request_stop)
    for block, order in enumerate(orders):
        for position, variant in enumerate(order):
            file = f'block-{block:04d}.{variant}.json'
            command = commands[file]
            print(f'block={block} position={position} variant={variant}', flush=True)
            result = run_trial(command, timeout, provenance['binaries'][variant]['sha256'])
            atomic_new(outdir / file, {'schema': 1, 'block': block, 'position': position,
                                        'variant': variant, 'order_seed': args.order_seed,
                                        'order': order, 'config': config, 'machine': machine,
                                        'artifacts': provenance, **result})
            if result['cleanup_error'] or result['artifact_error']:
                raise RuntimeError('stopping after unclean child or changed executable; failed raw record: '
                                   + str(outdir / file))
            if result['interrupted_signal'] is not None:
                return 128 + result['interrupted_signal']
            if args.stop_on_failure and not result['success']:
                raise RuntimeError('stopping after failed raw trial: ' + str(outdir / file))
    print('Manifest:', outdir / 'manifest.json')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except TrialStop as stop:
        sys.exit(128 + stop.signum)
