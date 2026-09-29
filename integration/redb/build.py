#!/usr/bin/env python3
"""Build the redb write-path variants from a pinned crate plus numbered patches.

The upstream redb 3.1.0 crate archive is verified against its crates.io SHA-256
(the checksum also locked in Cargo.lock), extracted, and patches/0*.patch are
applied in order. The patched tree is placed at the fixed Cargo path dependency
.worktree/redb-src/redb-3.1.0; an existing tree must be byte-identical to a fresh
application or the build stops. Three binaries are built with --locked:

  native      upstream crates.io redb, untouched          (variant native)
  patched     patched redb                                (refactored, bridge_mutex, mcs, uscl, fc, fc_pq)
  test_hooks  patched redb + dlock_test_hooks probes      (correctness gate only)

Every binary is instrumented (service_time: rdtscp around each write body) and the
patched ones build FC-PQ with the E0(b) fast path and its hit counter
(fcpq_fast_path, fcpq_fast_path_stat). --uninstrumented drops service_time and
fcpq_fast_path_stat (fast path kept) for the instrumentation-overhead check only;
run.py refuses such builds. build.json records each binary's requested harness
features, the libdlock/redb features Cargo resolved for it, and the features the
binary reports itself (--build-info).
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
HERE = ROOT / 'integration/redb'
MANIFEST = HERE / 'Cargo.toml'
PATCH_DIR = HERE / 'patches'
SOURCE_DIR = ROOT / '.worktree/redb-src/redb-3.1.0'
OUT = ROOT / '.worktree/redb'
CRATE = 'redb-3.1.0.crate'
CRATE_URL = 'https://static.crates.io/crates/redb/' + CRATE
CRATE_SHA256 = 'ae323eb086579a3769daa2c753bb96deb95993c534711e0dbe881b5192906a06'
PATCHED_VARIANTS = ['refactored', 'bridge_mutex', 'mcs', 'uscl', 'fc', 'fc_pq']
INSTRUMENTATION = ['service_time', 'fcpq_fast_path_stat']
BINARIES = {
    'native': {'features': ['native', 'service_time'], 'variants': ['native']},
    'patched': {'features': ['patched', 'service_time', 'fcpq_fast_path', 'fcpq_fast_path_stat'],
                'variants': PATCHED_VARIANTS},
    'test_hooks': {'features': ['test_hooks', 'service_time', 'fcpq_fast_path', 'fcpq_fast_path_stat'],
                   'variants': PATCHED_VARIANTS},
}
VARIANT_BINARY = {'native': 'native', **{variant: 'patched' for variant in PATCHED_VARIANTS}}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def digest(path):
    return sha256(Path(path).read_bytes())


def patches():
    found = sorted(PATCH_DIR.glob('[0-9][0-9][0-9][0-9]-*.patch'))
    if not found:
        raise RuntimeError('no numbered patches in ' + str(PATCH_DIR))
    return found


def tree_digest(directory):
    """Hash of every relative path and file content under directory."""
    h = hashlib.sha256()
    for path in sorted(p for p in Path(directory).rglob('*') if p.is_file()):
        h.update(str(path.relative_to(directory)).encode() + b'\0')
        h.update(hashlib.sha256(path.read_bytes()).digest())
    return h.hexdigest()


def crate_bytes():
    """Cargo's registry cache if present, else crates.io; both must match the pin."""
    cached = sorted(Path.home().glob('.cargo/registry/cache/*/' + CRATE))
    for path in cached:
        data = path.read_bytes()
        if sha256(data) == CRATE_SHA256:
            return data, str(path)
    with urllib.request.urlopen(CRATE_URL, timeout=60) as response:
        data = response.read()
    if sha256(data) != CRATE_SHA256:
        raise RuntimeError('downloaded redb crate does not match pinned SHA-256')
    return data, CRATE_URL


def locked_registry_checksum():
    """The checksum Cargo.lock pins for the registry redb used by `native`."""
    lines = (HERE / 'Cargo.lock').read_text().splitlines()
    for index, line in enumerate(lines):
        if line == 'name = "redb"' and lines[index + 1] == 'version = "3.1.0"':
            for follow in lines[index + 2:index + 5]:
                if follow.startswith('checksum = '):
                    return follow.split('"')[1]
    raise RuntimeError('Cargo.lock has no registry redb 3.1.0 checksum')


def apply_patches(destination):
    """Extract the pinned crate into destination and apply every numbered patch."""
    data, origin = crate_bytes()
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
        archive.extractall(destination, filter='data')
    tree = Path(destination) / 'redb-3.1.0'
    upstream = tree_digest(tree)
    for patch in patches():
        subprocess.run(['patch', '-p1', '--forward', '--batch', '--no-backup-if-mismatch',
                        '--quiet', '-i', str(patch)], cwd=tree, check=True)
    return tree, {'crate': CRATE, 'crate_sha256': CRATE_SHA256, 'crate_origin': origin,
                  'upstream_tree_sha256': upstream,
                  'patches': {p.name: digest(p) for p in patches()},
                  'patched_tree_sha256': tree_digest(tree)}


def prepare_source():
    """Create or verify the patched tree at the fixed Cargo path dependency."""
    if locked_registry_checksum() != CRATE_SHA256:
        raise RuntimeError('Cargo.lock registry redb checksum differs from the pinned crate')
    with tempfile.TemporaryDirectory(prefix='redb-patched-') as temp:
        tree, record = apply_patches(temp)
        if SOURCE_DIR.exists():
            if tree_digest(SOURCE_DIR) != record['patched_tree_sha256']:
                raise RuntimeError(f'{SOURCE_DIR} differs from the pinned crate plus current patches; '
                                   'remove it (it is generated) and rebuild')
            record['source_dir'] = 'verified existing'
        else:
            SOURCE_DIR.parent.mkdir(parents=True, exist_ok=True)
            shutil.copytree(tree, SOURCE_DIR)
            record['source_dir'] = 'created'
    record['source_path'] = str(SOURCE_DIR.relative_to(ROOT))
    return record


def harness_sources():
    """Every tracked input of the harness binaries except the generated redb tree."""
    files = [MANIFEST, HERE / 'Cargo.lock', Path(__file__).resolve(), *sorted((HERE / 'src').glob('*.rs'))]
    libdlock = ROOT / 'crates/libdlock'
    files += [libdlock / 'Cargo.toml', libdlock / 'build.rs']
    for pattern in ('*.rs', '*.c', '*.h'):
        files += sorted(libdlock.rglob(pattern))
    for pattern in ('*.c', '*.h'):
        files += sorted((ROOT / 'c').rglob(pattern))
    return {str(p.relative_to(ROOT)): digest(p) for p in files}


def output(command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def resolved_features(features):
    """Features Cargo resolves for libdlock and redb (patched or upstream) in this build."""
    metadata = json.loads(output(['cargo', 'metadata', '--manifest-path', str(MANIFEST), '--locked',
                                  '--filter-platform', 'x86_64-unknown-linux-gnu', '--format-version', '1', '--features', ','.join(features)]))
    names = {package['id']: package['name'] for package in metadata['packages']}
    resolved = {}
    for node in metadata['resolve']['nodes']:
        name = names[node['id']]
        if name in ('libdlock', 'redb', 'redb_transactions'):
            key = name if name != 'redb' or 'path+' in node['id'] else 'redb_upstream'
            resolved[key] = sorted(node['features'])
    # cargo metadata resolves the whole graph; keep only dependencies this build enables.
    enabled = set(resolved.get('redb_transactions', []))
    if 'native' in enabled:
        resolved.pop('libdlock', None)
        resolved.pop('redb', None)
    else:
        resolved.pop('redb_upstream', None)
    return resolved


def build(out, jobs, instrumented=True):
    out = out.resolve()
    if out.exists() and any(out.iterdir()):
        raise RuntimeError(f'{out} is not empty; use a fresh build directory')
    source = prepare_source()
    (out / 'bin').mkdir(parents=True)
    (out / 'logs').mkdir()
    binaries = {}
    for name, spec in BINARIES.items():
        features = [f for f in spec['features'] if instrumented or f not in INSTRUMENTATION]
        command = ['cargo', 'build', '--manifest-path', str(MANIFEST), '--release', '--locked',
                   '--bin', 'redb_transactions', '--features', ','.join(features),
                   '--target-dir', str(out / 'target' / name), '-j', str(jobs)]
        print('+', ' '.join(command), flush=True)
        log = out / 'logs' / (name + '.log')
        with log.open('xb') as stream:
            status = subprocess.run(command, cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT).returncode
        if status:
            raise RuntimeError(f'{name} build failed ({status}); log retained: {log}')
        snapshot = out / 'bin' / ('redb-' + name)
        shutil.copy2(out / 'target' / name / 'release/redb_transactions', snapshot)
        snapshot.chmod(0o555)
        reported = json.loads(output([str(snapshot), '--build-info']))
        if sorted(k for k, on in reported.items() if on) != sorted(
                {f for f in features} | ({'patched'} if 'test_hooks' in features else set())):
            raise RuntimeError(f'{name}: binary reports features {reported}, requested {features}')
        binaries[name] = {**spec, 'features': features, 'file': str(snapshot.relative_to(out)),
                          'sha256': digest(snapshot), 'command': command,
                          'resolved_features': resolved_features(features),
                          'reported_features': reported}
    manifest = {
        'schema': 2,
        'instrumented': instrumented,
        'redb_source': source,
        'harness_sources_sha256': harness_sources(),
        'binaries': binaries,
        'variant_binary': VARIANT_BINARY,
        'git_head': output(['git', 'rev-parse', 'HEAD']),
        'git_dirty_paths': output(['git', 'status', '--porcelain']).splitlines(),
        'rustc': output(['rustc', '-vV']).splitlines(),
        'cargo': output(['cargo', '--version']),
    }
    (out / 'build.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
    return manifest


def load_build(out):
    """Load build.json and verify it still describes the current sources and binaries."""
    out = out.resolve()
    manifest = json.loads((out / 'build.json').read_text())
    if manifest.get('schema') != 2:
        raise RuntimeError('unsupported build manifest (rebuild with the current build.py)')
    with tempfile.TemporaryDirectory(prefix='redb-verify-') as temp:
        _, fresh = apply_patches(temp)
    recorded = manifest['redb_source']
    for key in ('crate_sha256', 'upstream_tree_sha256', 'patches', 'patched_tree_sha256'):
        if recorded[key] != fresh[key]:
            raise RuntimeError(f'redb source/patch identity changed since build ({key}); rebuild')
    if tree_digest(SOURCE_DIR) != recorded['patched_tree_sha256']:
        raise RuntimeError(f'{SOURCE_DIR} changed since build; rebuild')
    if manifest['harness_sources_sha256'] != harness_sources():
        raise RuntimeError('harness/libdlock sources changed since build; rebuild into a fresh directory')
    for name, entry in manifest['binaries'].items():
        if digest(out / entry['file']) != entry['sha256']:
            raise RuntimeError(f'binary {name} changed since build')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--output-dir', type=Path, default=OUT)
    parser.add_argument('--jobs', type=int, default=8)
    parser.add_argument('--uninstrumented', action='store_true',
                        help='overhead check only: no service_time / fcpq_fast_path_stat')
    args = parser.parse_args()
    manifest = build(args.output_dir, args.jobs, instrumented=not args.uninstrumented)
    print(json.dumps({'status': 'ok', 'output_dir': str(args.output_dir.resolve()),
                      'instrumented': manifest['instrumented'],
                      'patched_tree_sha256': manifest['redb_source']['patched_tree_sha256'],
                      'binaries': {k: v['sha256'] for k, v in manifest['binaries'].items()}}))


if __name__ == '__main__':
    main()
