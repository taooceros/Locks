#!/usr/bin/env python3
"""Build pinned UpScaleDB single-operation native, control and bridge variants.

Existing checkouts require --resume; every tracked implementation is compared
byte-for-byte against a synthetic pinned-index application of its exact patches.
--reuse-build-root keeps verified standard DB libraries and their original
compilation identity read-only, while rebuilding the harness and Rust bridge.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
OUT = ROOT / '.worktree' / 'upscaledb'
SHA = 'cb124e1f91601872a7b3bd4da10e5fa97a8da86b'
NIXPKGS = 'https://github.com/NixOS/nixpkgs/archive/205fd4226592cc83fd4c0885a3e4c9c400efabb5.tar.gz'
UPSTREAM = 'https://github.com/cruppstahl/upscaledb.git'
IMPLEMENTATION = 'src/5upscaledb/upscaledb.cc'
PATCH = HERE / 'native-lock-timing.patch'
BRIDGE_PATCH = HERE / 'bridge-ops.patch'
HARNESS = HERE / 'native_harness.cc'
BRIDGE_KINDS = {'bridge_mutex': 0, 'fc': 1, 'fc_pq': 2, 'uscl': 3, 'cfl_local': 4,
                'spinlock': 5, 'mcs': 6, 'ticket': 7, 'clh': 8}
VARIANTS = ('native', 'refactored', *BRIDGE_KINDS, 'profile',
            *(name + '_profile' for name in BRIDGE_KINDS), 'test_hooks')


def source_kind(variant):
    return 'profile' if variant == 'profile' else (
        'native' if variant == 'native' else
        'refactored_test' if variant == 'test_hooks' else 'refactored')


def source_patches(kind):
    if kind == 'profile':
        return (PATCH,)
    return (BRIDGE_PATCH,) if kind not in ('native', 'profile') else ()

CONFIGURE_FLAGS = ('--disable-remote', '--disable-java', '--disable-encryption',
                   '--without-tcmalloc', '--without-berkeleydb')


def run(args, cwd=ROOT, env=None):
    print('+', ' '.join(str(a) for a in args), flush=True)
    return subprocess.run([str(a) for a in args], cwd=cwd, env=env, check=True,
                          text=True)


def output(args, cwd=ROOT, env=None):
    return subprocess.check_output([str(a) for a in args], cwd=cwd, env=env)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def rust_sources_files():
    files = [ROOT / 'Cargo.toml', ROOT / 'Cargo.lock', HERE / 'bridge.h']
    files.extend((ROOT / '.cargo').glob('*.toml'))
    # libdlock/build.rs compiles C code and bindgen reads wrapper + transitive
    # project headers. Hash the whole local C input tree rather than guessing
    # which headers a selected feature might include.
    for suffix in ('*.c', '*.h'):
        files.extend((ROOT / 'c').rglob(suffix))
    for crate in ('libdlock', 'upscaledb-bridge'):
        directory = ROOT / 'crates' / crate
        files.append(directory / 'Cargo.toml')
        files.extend(directory.glob('src/**/*.rs'))
        files.extend(directory.glob('binding/**/*.h'))
        build_script = directory / 'build.rs'
        if build_script.is_file():
            files.append(build_script)
    return sorted(files)


def rust_sources_digest():
    checksum = hashlib.sha256()
    for file in rust_sources_files():
        checksum.update(str(file.relative_to(ROOT)).encode())
        checksum.update(file.read_bytes())
    return checksum.hexdigest()


def nix(attr):
    command = ['nix-build', NIXPKGS, '-A', attr, '--no-out-link']
    print('+', ' '.join(command), flush=True)
    text = output(command).decode()
    result = Path(text.strip().splitlines()[-1])
    if not result.is_absolute() or not result.exists():
        raise RuntimeError('nix-build did not return a store output: ' + text)
    print(attr, '=', result, flush=True)
    return result


def shell(*args, cwd=ROOT):
    run(['devenv', 'shell', '--', *args], cwd=cwd)


def git(source, *args, env=None):
    clean = os.environ.copy()
    for key in ('GIT_INDEX_FILE', 'GIT_DIR', 'GIT_WORK_TREE',
                'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES'):
        clean.pop(key, None)
    clean['GIT_OPTIONAL_LOCKS'] = '0'
    if env:
        clean.update(env)
    return output(['git', *args], cwd=source, env=clean)


def verify_source(source, variant, provenance_root=None):
    if git(source, 'rev-parse', 'HEAD').decode().strip() != SHA:
        raise RuntimeError('UpScaleDB source HEAD differs from pinned SHA: ' + str(source))
    if git(source, 'ls-files', '--unmerged', '-z'):
        raise RuntimeError('Unmerged source tree: ' + str(source))
    changed = {p.decode() for p in git(source, 'diff', '--name-only', '-z', 'HEAD', '--').split(b'\0') if p}
    permitted = {'config.h.in'} | ({IMPLEMENTATION} if variant != 'native' else set())
    if changed - permitted:
        raise RuntimeError('Unrelated tracked source edits: ' + ', '.join(sorted(changed - permitted)))
    untracked = [p.decode() for p in git(source, 'ls-files', '--others',
                                          '--exclude-standard', '-z').split(b'\0') if p]
    stray_code = [p for p in untracked if p.startswith(('src/', 'include/'))
                  and Path(p).suffix in ('.cc', '.cpp', '.h', '.hpp')]
    if stray_code:
        raise RuntimeError('Unrelated untracked source files: ' + ', '.join(sorted(stray_code)))
    template = source / 'config.h.in'
    if 'config.h.in' in changed and (not template.is_file() or
            b'Generated from configure.ac by autoheader' not in template.read_bytes()[:256]):
        raise RuntimeError('Unexpected change to generated config.h.in')
    previous = (OUT if provenance_root is None else provenance_root) / ('build-' + variant + '.json')
    if previous.is_file():
        recorded = json.loads(previous.read_text()).get('source_verification', {})
        template_hash = recorded.get('generated_config_h_in_sha256')
        if template_hash and (not template.is_file() or digest(template) != template_hash):
            raise RuntimeError('Generated source template changed since verified manifest: ' + str(template))
    with tempfile.TemporaryDirectory(prefix='upscaledb-expected-index-') as temp:
        # Applying into an alternate index can still write Git objects.
        # Keep both index and newly synthesized blobs outside the frozen DB.
        objects = Path(git(source, 'rev-parse', '--git-path', 'objects').decode().strip())
        if not objects.is_absolute():
            objects = source / objects
        temporary_objects = Path(temp) / 'objects'
        temporary_objects.mkdir()
        index_env = {'GIT_INDEX_FILE': str(Path(temp) / 'index'),
                     'GIT_OBJECT_DIRECTORY': str(temporary_objects),
                     'GIT_ALTERNATE_OBJECT_DIRECTORIES': str(objects.resolve(strict=True))}
        git(source, 'read-tree', SHA, env=index_env)
        for patch in source_patches(variant):
            git(source, 'apply', '--cached', '--whitespace=nowarn', str(patch), env=index_env)
        expected = git(source, 'show', ':' + IMPLEMENTATION, env=index_env)
    actual = source / IMPLEMENTATION
    if not actual.is_file() or actual.read_bytes() != expected:
        raise RuntimeError('Tracked implementation differs from intended ' + variant + ' variant: ' + str(actual))
    result = {'head': SHA, 'tracked_changes': sorted(changed),
              'implementation_sha256': digest(actual),
              'generated_config_h_in_sha256': digest(template) if template.is_file() else None,
              'profile_patch_sha256': digest(PATCH) if variant == 'profile' else None,
              'bridge_patch_sha256': digest(BRIDGE_PATCH) if variant not in ('native', 'profile') else None}
    return result


def verify_library(source, variant, boost_dev, gcc, boost, source_info, require_provenance,
                   provenance_root=None, integration_dir=None):
    provenance_root = OUT if provenance_root is None else provenance_root
    integration_dir = HERE if integration_dir is None else integration_dir
    lib = source / 'src/.libs/libupscaledb.so'
    status = source / 'config.status'
    header = source / 'config.h'
    if not lib.is_file() or not status.is_file() or not header.is_file():
        raise RuntimeError('Existing configured library, config.status and config.h required: ' + str(source))
    config = status.read_text(errors='replace')
    expected_flags = ('--with-boost=' + str(boost_dev), *CONFIGURE_FLAGS,
                      'CC=' + str(gcc / 'bin/gcc'), 'CXX=' + str(gcc / 'bin/g++'),
                      'CFLAGS=-O3 -g -DNDEBUG', 'LDFLAGS=-L' + str(boost / 'lib'))
    if variant in ('refactored', 'refactored_test'):
        cppflags = '-I' + str(integration_dir)
        if variant == 'refactored_test':
            cppflags += ' -DDLOCK_BRIDGE_TEST_HOOKS'
        expected_flags += ('CPPFLAGS=' + cppflags,)
    if any(flag not in config for flag in expected_flags):
        raise RuntimeError('Existing configure flags/toolchain do not match required upstream configuration')
    with lib.open('rb') as stream:
        if stream.read(4) != b'\x7fELF':
            raise RuntimeError('Existing library is not an ELF shared object: ' + str(lib))
    # The profile-only exported symbol prevents accidentally using a native
    # binary (or vice versa) when a checkout has been built in the wrong mode.
    symbols = output(['devenv', 'shell', '--', 'nm', '-D', '--defined-only', str(lib)]).decode().splitlines()
    has_profile = any(line.split() and line.split()[-1] == 'ups_native_lock_sample' for line in symbols)
    if has_profile != (variant == 'profile'):
        raise RuntimeError('Existing library profile instrumentation does not match ' + variant)
    if any(line.split() and line.split()[-1] == 'ups_dlock_private_native_batch'
           for line in symbols):
        raise RuntimeError('Existing library exposes unsupported batch ABI: ' + variant)
    result = {'shared_object': str(lib.resolve(strict=True)), 'shared_object_sha256': digest(lib),
              'config_status_sha256': digest(status), 'config_h_sha256': digest(header)}
    if require_provenance:
        # Several harness variants share one library. Accept an exact record
        # from any of them, but never infer provenance from ELF/config alone.
        matched = False
        for name in VARIANTS:
            previous = provenance_root / ('build-' + name + '.json')
            if not previous.is_file():
                continue
            try:
                recorded = json.loads(previous.read_text())
            except (OSError, ValueError):
                continue
            if not isinstance(recorded, dict):
                continue
            if (recorded.get('schema') != 2 or recorded.get('pinned_sha') != SHA or
                    recorded.get('variant') != name or recorded.get('source_kind') != variant or
                    recorded.get('source') != str(source) or
                    recorded.get('source_verification') != source_info or
                    recorded.get('library_verification') != result):
                continue
            if variant not in ('native', 'profile'):
                hashes = recorded.get('hashes')
                if not isinstance(hashes, dict) or any(
                        hashes.get(key) != digest(integration_dir / filename)
                        for key, filename in (('private_ops_sha256', 'private_ops.h'),
                                              ('bridge_h_sha256', 'bridge.h'))):
                    continue
            matched = True
            break
        if not matched:
            raise RuntimeError('No matching prior library provenance; rebuild rather than reuse: ' + str(source))
    return result


def reuse_manifest(root, kind):
    name = {'native': 'native', 'profile': 'profile', 'refactored': 'fc',
            'refactored_test': 'test_hooks'}[kind]
    path = root / ('build-' + name + '.json')
    recorded = json.loads(path.read_text())
    if (recorded.get('schema') != 2 or recorded.get('pinned_sha') != SHA or
            recorded.get('nixpkgs') != NIXPKGS or recorded.get('variant') != name or
            recorded.get('source_kind') != kind or
            recorded.get('source') != str(root / ('src-' + kind))):
        raise RuntimeError('Incompatible frozen database manifest: ' + str(path))
    return path, recorded


def verify_bridge_header_extension(previous, current):
    """Permit only comments/whitespace and appended explicit backend IDs.

    Everything else, including structures, function signatures, statuses and
    preprocessor directives, must retain its C token sequence. Both complete
    header hashes remain separately recorded; this is not a hash bypass.
    """
    def split(text):
        text = re.sub(r'/\*.*?\*/|//[^\n]*', '', text, flags=re.S)
        matches = list(re.finditer(r'\benum\s+dlock_bridge_kind\s*\{([^{}]*)\}\s*;', text))
        if len(matches) != 1:
            raise RuntimeError('Cannot establish bridge backend enum compatibility')
        match = matches[0]
        entries = []
        for item in match.group(1).rstrip().rstrip(',').split(','):
            value = re.fullmatch(r'\s*(DLOCK_BRIDGE_[A-Z_]+)\s*=\s*(\d+)\s*', item)
            if value is None:
                raise RuntimeError('Backend IDs must be explicit decimal constants')
            entries.append((value.group(1), int(value.group(2))))
        rest = text[:match.start()] + text[match.end():]
        # Preserve directive line boundaries as well as ordinary C tokens.
        directives = [line.strip() for line in rest.splitlines() if line.lstrip().startswith('#')]
        return entries, re.findall(r'[A-Za-z_]\w*|\d+|[^\s]', rest), directives

    old, old_tokens, old_directives = split(previous)
    new, new_tokens, new_directives = split(current)
    if (old_tokens != new_tokens or old_directives != new_directives or
            new[:len(old)] != old or len({name for name, _ in new}) != len(new) or
            len({number for _, number in new}) != len(new)):
        raise RuntimeError('Frozen database bridge ABI differs beyond appended backend IDs')
    return {'rule': 'identical C ABI tokens except append-only backend enum',
            'previous_backend_ids': dict(old), 'current_backend_ids': dict(new)}


def database_reuse_provenance(root, kind, boost, boost_dev, gcc):
    path, recorded = reuse_manifest(root, kind)
    toolchain = recorded['toolchain']
    for key, expected in (('boost_store', boost), ('boost_dev_store', boost_dev),
                          ('gcc_store', gcc), ('cc', gcc / 'bin/gcc'),
                          ('cxx', gcc / 'bin/g++')):
        if toolchain.get(key) != str(expected):
            raise RuntimeError('Frozen database compiler/Boost mismatch: ' + str(path))
    for name in ('cc', 'cxx'):
        if toolchain.get(name + '_sha256') != digest(Path(toolchain[name])):
            raise RuntimeError('Frozen database compiler changed: ' + toolchain[name])
    if (toolchain.get('cflags') != '-O3 -g -DNDEBUG' or
            toolchain.get('ldflags') != '-L' + str(boost / 'lib') or
            toolchain.get('nix_cflags_compile_unset') is not True or
            toolchain.get('nix_ldflags_unset') is not True):
        raise RuntimeError('Frozen database build flags differ: ' + str(path))
    hashes = recorded['hashes']
    headers = []
    for arg in recorded['build']['harness_command']:
        if not arg.startswith('-I'):
            continue
        directory = Path(arg[2:])
        if all((directory / filename).is_file() and
               digest(directory / filename) == hashes.get(key)
               for filename, key in (('bridge.h', 'bridge_h_sha256'),
                                     ('private_ops.h', 'private_ops_sha256'))):
            headers.append(directory)
    if len(headers) != 1:
        raise RuntimeError('Frozen database compilation headers are missing or changed: ' + str(path))
    integration_dir = headers[0]
    if digest(HERE / 'private_ops.h') != hashes['private_ops_sha256']:
        raise RuntimeError('Private database callback ABI changed; rebuild database')
    compatibility = verify_bridge_header_extension(
        (integration_dir / 'bridge.h').read_text(), (HERE / 'bridge.h').read_text())
    expected_cppflags = '' if kind in ('native', 'profile') else '-I' + str(integration_dir)
    if kind == 'refactored_test':
        expected_cppflags += ' -DDLOCK_BRIDGE_TEST_HOOKS'
    if toolchain.get('cppflags') != expected_cppflags:
        raise RuntimeError('Frozen database header search path differs: ' + str(path))
    for key, filename in (('configure_sha256', 'configure'),
                          ('config_h_in_sha256', 'config.h.in')):
        if hashes.get(key) != digest(Path(recorded['source']) / filename):
            raise RuntimeError('Frozen database configuration input changed: ' + filename)
    return {
        'build_manifest': str(path), 'build_manifest_sha256': digest(path),
        'source_verification': recorded['source_verification'],
        'library_verification': recorded['library_verification'],
        'toolchain': toolchain,
        'integration_directory': str(integration_dir),
        'compilation_header_hashes': {
            'bridge_h_sha256': hashes['bridge_h_sha256'],
            'private_ops_sha256': hashes['private_ops_sha256'],
        },
        'current_bridge_header_sha256': digest(HERE / 'bridge.h'),
        'header_compatibility': compatibility,
        'scope': 'DB shared object only; harness and Rust archive use current checkout inputs',
    }, integration_dir


def build(variant, jobs, boost, boost_dev, gcc, resume, harness_only, built_sources,
          built_archives, reuse_build_root=None, source_repository=UPSTREAM):
    kind = source_kind(variant)
    provenance_root = OUT if reuse_build_root is None else reuse_build_root
    source = provenance_root / ('src-' + kind)
    database_reuse = None
    integration_dir = HERE
    if reuse_build_root is not None:
        database_reuse, integration_dir = database_reuse_provenance(
            reuse_build_root, kind, boost, boost_dev, gcc)
    if reuse_build_root is not None and (not harness_only or not resume):
        raise RuntimeError('Frozen database reuse requires --resume --harness-only')
    reused = source.exists()
    if reused:
        if not resume and kind not in built_sources:
            raise RuntimeError('Existing source requires explicit --resume (never removed or overwritten): ' + str(source))
        if not source.is_dir():
            raise RuntimeError('Existing source path is not a directory: ' + str(source))
        source_info = verify_source(source, kind, provenance_root)
    elif harness_only:
        raise RuntimeError('--harness-only requires an existing verified source and library: ' + str(source))
    else:
        run(['git', 'clone', '--no-checkout', source_repository, source])
        run(['git', 'checkout', '--detach', SHA], cwd=source)
        for patch in source_patches(kind):
            run(['git', 'apply', '--whitespace=nowarn', patch], cwd=source)
        source_info = verify_source(source, kind)

    clean_env = os.environ.copy()
    clean_env.pop('NIX_CFLAGS_COMPILE', None)
    clean_env.pop('NIX_LDFLAGS', None)
    clean_env.update(CC=str(gcc / 'bin/gcc'), CXX=str(gcc / 'bin/g++'),
                     CFLAGS='-O3 -g -DNDEBUG', LDFLAGS='-L' + str(boost / 'lib'))
    if kind != 'native' and kind != 'profile':
        # Upstream configure.ac derives CXXFLAGS from CFLAGS; CPPFLAGS survives.
        clean_env['CPPFLAGS'] = '-I' + str(integration_dir)
        if kind == 'refactored_test':
            clean_env['CPPFLAGS'] += ' -DDLOCK_BRIDGE_TEST_HOOKS'
    configure = ['env', '-C', source, '-u', 'NIX_CFLAGS_COMPILE', '-u', 'NIX_LDFLAGS',
                 'CC=' + clean_env['CC'], 'CXX=' + clean_env['CXX'],
                 'CFLAGS=' + clean_env['CFLAGS']]
    if kind not in ('native', 'profile'):
        configure.append('CPPFLAGS=' + clean_env['CPPFLAGS'])
    configure += ['LDFLAGS=' + clean_env['LDFLAGS'],
                  './configure', '--with-boost=' + str(boost_dev), *CONFIGURE_FLAGS]
    make = [['env', '-u', 'NIX_CFLAGS_COMPILE', '-u', 'NIX_LDFLAGS',
             'make', '-C', source / directory, '-j' + str(jobs)]
            for directory in ('3rdparty', 'src')]
    # Existing gate libraries are immutable evidence; only compile a new
    # harness against them. A fresh clone still receives a full build.
    built_now = (not harness_only and kind not in built_sources and
                 (not reused or kind not in ('native', 'profile')))
    if built_now:
        shell('autoreconf', '--force', '--install', source)
        shell(*configure)
        for command in make:
            shell(*command)
        source_info = verify_source(source, kind)
        built_sources.add(kind)
    lib_info = verify_library(source, kind, boost_dev, gcc, boost, source_info, not built_now,
                              provenance_root, integration_dir)
    if database_reuse is not None and (
            database_reuse['source_verification'] != source_info or
            database_reuse['library_verification'] != lib_info):
        raise RuntimeError('Frozen database source/library differs from authoritative manifest')
    lib = source / 'src/.libs'
    binary = OUT / ('upscaledb-' + variant)
    if variant in ('native', 'profile'):
        prior = OUT / ('build-' + variant + '.json')
        if binary.exists() and prior.exists():
            saved = OUT / 'gate-permuted-preserved'
            saved.mkdir(exist_ok=True)
            for path in (binary, prior):
                copy = saved / path.name
                if not copy.exists():
                    shutil.copy2(path, copy)
    args = [str(gcc / 'bin/g++'), '-std=c++17', '-O3', '-g', '-DNDEBUG', '-pthread',
            '-I' + str(source / 'include'), '-I' + str(HERE)]
    if variant == 'profile':
        args.append('-DUPS_NATIVE_PROFILE=1')
    bridge_kind = 2 if variant == 'test_hooks' else BRIDGE_KINDS.get(
        variant.removesuffix('_profile'))
    profile = variant.endswith('_profile')
    if variant == 'refactored':
        args.append('-DUPS_REFACTORED=1')
    if bridge_kind is not None:
        args.append('-DUPS_BRIDGE_KIND=' + str(bridge_kind))
        if profile:
            args.append('-DUPS_BRIDGE_PROFILE=1')
        if variant == 'test_hooks':
            args.append('-DDLOCK_BRIDGE_TEST_HOOKS=1')
    args.extend([str(HARNESS), '-o', str(binary),
                 '-L' + str(lib), '-L' + str(boost / 'lib'),
                 '-Wl,-rpath,' + str(lib), '-Wl,-rpath,' + str(boost / 'lib')])
    for component in ('murmurhash3', 'liblzf', 'libfor', 'libvbyte',
                      'simdcomp', 'streamvbyte'):
        args.append('-Wl,-rpath,' + str(source / '3rdparty' / component / '.libs'))
    rust_info = None
    if bridge_kind is not None:
        features = ('test-hooks',) if variant == 'test_hooks' else ('profile',) if profile else ()
        compiler_info = {
            'rustc_vv': output(['devenv', 'shell', '--', 'rustc', '-Vv']).decode().strip(),
            'cargo_version': output(['devenv', 'shell', '--', 'cargo', '-V']).decode().strip(),
        }
        target = OUT / ('cargo-' + ('test-hooks' if variant == 'test_hooks' else
                                    'profile' if profile else 'primary'))
        archive = target / 'release/libupscaledb_bridge.a'
        archive_manifest = target / 'archive.json'
        cargo = ['devenv', 'shell', '--', 'env', 'CARGO_TARGET_DIR=' + str(target),
                 'cargo', 'build', '--release', '--package', 'upscaledb-bridge',
                 '--no-default-features', '--jobs', str(jobs)]
        if features:
            cargo += ['--features', ','.join(features)]
        if archive.is_file() or archive_manifest.exists():
            if ((not resume and archive not in built_archives)
                    or not archive.is_file() or not archive_manifest.is_file()):
                raise RuntimeError('Existing Rust archive requires --resume and its manifest: ' + str(archive))
            recorded = json.loads(archive_manifest.read_text())
            if (recorded.get('archive_sha256') != digest(archive) or
                    recorded.get('features') != list(features) or
                    recorded.get('source_root') != str(ROOT) or
                    recorded.get('bridge_h_sha256') != digest(HERE / 'bridge.h') or
                    recorded.get('rust_sources_sha256') != rust_sources_digest() or
                    recorded.get('compiler') != compiler_info):
                raise RuntimeError('Rust staticlib/features/ABI differ from recorded archive: ' + str(archive))
        else:
            run(cargo)
            recorded = {'archive_sha256': digest(archive), 'features': list(features),
                        'bridge_h_sha256': digest(HERE / 'bridge.h'),
                        'rust_sources_sha256': rust_sources_digest(),
                        'compiler': compiler_info, 'command': cargo, 'source_root': str(ROOT)}
            archive_manifest.write_text(json.dumps(recorded, indent=2, sort_keys=True) + '\n')
            built_archives.add(archive)
        rust_info = {'archive': str(archive), **recorded}
        args.append(str(archive))
    args.extend(['-lupscaledb', '-lboost_thread', '-lboost_system',
                 '-ldl', '-lrt', '-lm', '-lutil', '-pthread'])
    run(args, env=clean_env)
    baseline = None
    if bridge_kind == 3:
        baseline = {
            'name': 'uscl', 'language': 'C via Rust FFI',
            'source': 'c/u-scl/fairlock.h', 'sha256': digest(ROOT / 'c/u-scl/fairlock.h'),
            'common_sha256': digest(ROOT / 'c/u-scl/common.h'),
            'weight_per_live_worker': 1024, 'scheduler_nice_used': False,
            'slice_cycles': 4800000, 'cycle_per_us_compile_time': 2400,
            'slice_nominal_ms': 2,
            'prior_host_tsc_ticks_per_ns': 2.19995,
            'estimated_prior_host_slice_ms': 4.8 / 2.19995,
            'timing_caveat': 'fixed 2400 cycles/us assumes a 2.4GHz invariant TSC; '
                             'prior host gate measured ~2.19995 cycles/ns, implying '
                             '~2.18ms real slice; recalibrate on actual trial host. '
                             'Kernel/scheduler behavior is not CFS-paper fidelity',
            'tls': 'pthread key per stable lock, live-worker weighted total, '
                   'destructor after worker exit/join; destroy deletes key'
        }
    elif bridge_kind == 4:
        baseline = {
            'name': 'cfl_local', 'language': 'local Rust port of adapted C fairnumas',
            'source': 'crates/libdlock/src/dlock2/cfl.rs',
            'sha256': digest(ROOT / 'crates/libdlock/src/dlock2/cfl.rs'),
            'local_c_source': 'c/cfl/cfl.c',
            'local_c_sha256': digest(ROOT / 'c/cfl/cfl.c'),
            'author_source': 'https://github.com/jonggyup/Completely-Fair-Locking',
            'author_revision_audited': '2b7c51465847459f76b1bfd988a0c219756b3728',
            'fidelity': 'local proxy, NOT verified faithful published implementation',
            'topology': 'RDTSCP AUX low12 logical CPU ID mapped to real sysfs '
                        'cpu*/node* links once at startup (NOT original CPU modulo '
                        'node count); fixed 256-CPU/16-node vLHT, fail-stop if '
                        'mapping absent or indices exceed capacity',
            'vLHT': 'process-wide atomics; ALLOWED_NODE and vLHT persist across '
                    'handle destruction/recreation and warmup in same process '
                    '(no unsafe per-handle reset); local C shuffle algorithm; '
                    'standard = runtime_checker_node[nid] / 16 (inherited '
                    'hardcoded parameter, not auto-scaled to 32 physical cores/node)'
        }
    elif bridge_kind in (5, 6, 7, 8):
        name, relative = {
            5: ('spinlock', 'crates/libdlock/src/spin_lock.rs'),
            6: ('mcs', 'crates/libdlock/src/dlock2/mcs.rs'),
            7: ('ticket', 'crates/libdlock/src/dlock2/ticket.rs'),
            8: ('clh', 'crates/libdlock/src/dlock2/clh.rs'),
        }[bridge_kind]
        baseline = {
            'name': name, 'language': 'existing Rust RawMutex via DLock2Wrapper',
            'source': relative, 'sha256': digest(ROOT / relative),
            'wrapper_source': 'crates/libdlock/src/dlock2/spinlock.rs',
            'wrapper_sha256': digest(ROOT / 'crates/libdlock/src/dlock2/spinlock.rs'),
            'execution': 'synchronous requester; lock and unlock on the same worker',
            'lifetime': 'all callers joined before exclusive handle destruction',
        }
        if bridge_kind == 8:
            baseline['allocation_caveat'] = (
                'existing CLH leaks one 128-byte queue node per retained ThreadLocal '
                'registration slot plus one sentinel per used handle; process exit '
                'reclaims them, handle destruction does not')
    manifest = {
        'schema': 2, 'variant': variant, 'source_kind': kind, 'source': str(source),
        'source_reused': reused, 'source_verification': source_info,
        'source_clone_from': source_repository if not reused else None,
        'baseline': baseline,
        'database_reuse': database_reuse,
        'library_verification': lib_info, 'rust_staticlib': rust_info,
        'upstream': UPSTREAM, 'pinned_sha': SHA, 'nixpkgs': NIXPKGS,
        'toolchain': {'gcc_store': str(gcc), 'cc': clean_env['CC'], 'cxx': clean_env['CXX'],
                      'cc_sha256': digest(Path(clean_env['CC'])),
                      'cxx_sha256': digest(Path(clean_env['CXX'])),
                      'boost_store': str(boost), 'boost_dev_store': str(boost_dev),
                      'cflags': clean_env['CFLAGS'],
                      'cxxflags': clean_env['CFLAGS'] + ' -std=c++0x',
                      'cppflags': clean_env.get('CPPFLAGS', ''),
                      'ldflags': clean_env['LDFLAGS'],
                      'nix_cflags_compile_unset': True, 'nix_ldflags_unset': True},
        'build': {'harness_only': harness_only, 'autoreconf_executed': built_now,
                  'configure_executed': built_now, 'make_executed': built_now,
                  'autoreconf_command': ['devenv', 'shell', '--', 'autoreconf', '--force', '--install', str(source)],
                  'configure_command': ['devenv', 'shell', '--', *map(str, configure)],
                  'make_commands': [['devenv', 'shell', '--', *map(str, command)] for command in make],
                  'harness_command': args, 'harness_cxx_standard': 'c++17',
                  'upstream_cxx_standard': 'c++0x' if kind not in ('native', 'profile') else 'upstream_configured_default',
                  'jobs': jobs},
        'hashes': {'build_py_sha256': digest(HERE / 'build.py'),
                   'harness_sha256': digest(HARNESS), 'binary_sha256': digest(binary),
                   'private_ops_sha256': digest(HERE / 'private_ops.h'),
                   'bridge_h_sha256': digest(HERE / 'bridge.h'),
                   'configure_sha256': digest(source / 'configure'),
                   'config_h_in_sha256': digest(source / 'config.h.in')},
        'binary': str(binary),
    }
    if database_reuse is not None:
        # This field describes DB compilation, not the fresh harness include
        # path (recorded in harness_command) or fresh Rust staticlib inputs.
        manifest['toolchain'] = database_reuse['toolchain']
    manifest_file = OUT / ('build-' + variant + '.json')
    manifest_file.write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
    print('Built', binary, 'from', SHA, '(', variant, '); manifest:', manifest_file, flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--variant', choices=(*VARIANTS, 'all'), default='all')
    parser.add_argument('--variants', help='comma-separated exact variant names (excludes all)')
    parser.add_argument('--output-root', type=Path, default=None,
                        help='separate output directory; default preserves legacy .worktree/upscaledb')
    parser.add_argument('--source-repository', default=UPSTREAM,
                        help='pinned UpScaleDB Git repository URL or local clone')
    parser.add_argument('--jobs', type=int, default=8)
    parser.add_argument('--resume', action='store_true',
                        help='verify and reuse existing pinned source; clone missing variants')
    parser.add_argument('--reuse-build-root', type=Path,
                        help='read-only verified DB libraries/manifests from another build root; '
                             'requires --resume --harness-only and a separate output root')
    parser.add_argument('--harness-only', action='store_true',
                        help='requires --resume and verified existing configured library')
    args = parser.parse_args()
    if args.variants and args.variant != 'all':
        parser.error('--variants cannot be combined with a non-default --variant')
    if args.variants:
        selected = tuple(args.variants.split(','))
        if not selected or any(name not in VARIANTS for name in selected) or len(set(selected)) != len(selected):
            parser.error('--variants must list distinct known variant names')
    else:
        selected = VARIANTS[:-1] if args.variant == 'all' else (args.variant,)
    global OUT
    if args.output_root is not None:
        OUT = args.output_root.resolve()
    if not 1 <= args.jobs <= 256:
        parser.error('--jobs must be in [1, 256]')
    if args.harness_only and not args.resume:
        parser.error('--harness-only requires --resume')
    reuse_root = args.reuse_build_root.resolve() if args.reuse_build_root is not None else None
    if reuse_root is not None and (not args.harness_only or not args.resume):
        parser.error('--reuse-build-root requires --resume --harness-only')
    if reuse_root is not None and (OUT == reuse_root or OUT.is_relative_to(reuse_root)):
        parser.error('--reuse-build-root must remain read-only; choose a separate --output-root')
    OUT.mkdir(parents=True, exist_ok=True)
    if reuse_root is None:
        boost, boost_dev, gcc = nix('boost'), nix('boost.dev'), nix('gcc')
    else:
        _, recorded = reuse_manifest(reuse_root, source_kind(selected[0]))
        toolchain = recorded['toolchain']
        boost, boost_dev, gcc = (Path(toolchain[key]) for key in
                                 ('boost_store', 'boost_dev_store', 'gcc_store'))
    built_sources = set()
    built_archives = set()
    # --variants can select only the desired primary/profile/control builds.
    for variant in selected:
        build(variant, args.jobs, boost, boost_dev, gcc, args.resume, args.harness_only,
              built_sources, built_archives, reuse_root, args.source_repository)


if __name__ == '__main__':
    main()
