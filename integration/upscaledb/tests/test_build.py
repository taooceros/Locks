"""Library reuse must retain a source-to-binary provenance chain."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from integration.upscaledb.core import build


class LibraryProvenance(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / 'src-refactored'
        self.library = self.source / 'src/.libs/libupscaledb.so'
        self.library.parent.mkdir(parents=True)
        self.library.write_bytes(b'\x7fELFfixture')
        self.here = self.root / 'integration'
        self.here.mkdir()
        for name in ('private_ops.h', 'bridge.h'):
            (self.here / name).write_text('original ABI\n')
        self.boost_dev, self.gcc, self.boost = map(Path, ('/boost-dev', '/gcc', '/boost'))
        flags = ('--with-boost=/boost-dev', *build.CONFIGURE_FLAGS,
                 'CC=/gcc/bin/gcc', 'CXX=/gcc/bin/g++', 'CFLAGS=-O3 -g -DNDEBUG',
                 'LDFLAGS=-L/boost/lib', 'CPPFLAGS=-I' + str(self.here))
        (self.source / 'config.status').write_text('\n'.join(flags))
        (self.source / 'config.h').write_text('configured\n')
        self.info = {'head': build.SHA, 'implementation_sha256': 'verified-source',
                     'bridge_patch_sha256': 'verified-patch'}
        for name, value in (('OUT', self.root), ('HERE', self.here)):
            patcher = patch.object(build, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        # Symbol discovery is orthogonal to the provenance policy under test.
        patcher = patch.object(build, 'output', return_value=b'')
        patcher.start()
        self.addCleanup(patcher.stop)
        self.library_info = self.verify(require_provenance=False)

    def verify(self, require_provenance=True):
        return build.verify_library(self.source, 'refactored', self.boost_dev,
                                    self.gcc, self.boost, self.info, require_provenance)

    def record(self):
        manifest = {'schema': 2, 'pinned_sha': build.SHA, 'variant': 'fc',
                    'source_kind': 'refactored', 'source': str(self.source),
                    'source_verification': self.info,
                    'library_verification': self.library_info,
                    'hashes': {'private_ops_sha256': build.digest(self.here / 'private_ops.h'),
                               'bridge_h_sha256': build.digest(self.here / 'bridge.h')}}
        (self.root / 'build-fc.json').write_text(json.dumps(manifest))

    def test_missing_manifest_does_not_establish_library_provenance(self):
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            self.verify()

    def test_shared_library_can_use_matching_fc_manifest(self):
        self.record()
        self.assertEqual(self.verify(), self.library_info)

    def test_changed_source_or_library_cannot_reuse_record(self):
        self.record()
        self.info = {**self.info, 'implementation_sha256': 'new-source'}
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            self.verify()
        self.info['implementation_sha256'] = 'verified-source'
        self.library.write_bytes(b'\x7fELFdifferent-library')
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            self.verify()

    def test_changed_callback_header_invalidates_library_record(self):
        self.record()
        (self.here / 'private_ops.h').write_text('changed ABI\n')
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            self.verify()


class BridgeHeaderCompatibility(unittest.TestCase):
    OLD = """#ifndef BRIDGE_H
#define BRIDGE_H
enum dlock_bridge_kind {
  DLOCK_BRIDGE_MUTEX = 0,
  DLOCK_BRIDGE_FC = 1
};
struct metrics { uint64_t ticks; uint32_t enabled; };
int32_t execute(void *handle, struct metrics *out);
#endif
"""

    def extended(self):
        return self.OLD.replace('DLOCK_BRIDGE_FC = 1',
                                'DLOCK_BRIDGE_FC = 1, DLOCK_BRIDGE_SPINLOCK = 5')

    def test_appended_backend_preserves_existing_callback_abi(self):
        result = build.verify_bridge_header_extension(
            self.OLD, self.extended().replace('struct metrics', '/* unchanged ABI */ struct metrics'))
        self.assertEqual(result['previous_backend_ids'],
                         {'DLOCK_BRIDGE_MUTEX': 0, 'DLOCK_BRIDGE_FC': 1})
        self.assertEqual(result['current_backend_ids'],
                         {'DLOCK_BRIDGE_MUTEX': 0, 'DLOCK_BRIDGE_FC': 1, 'DLOCK_BRIDGE_SPINLOCK': 5})

    def test_struct_and_callback_signature_changes_require_database_rebuild(self):
        for changed in (self.extended().replace('uint64_t ticks', 'uint32_t ticks'),
                        self.extended().replace('struct metrics *out', 'const struct metrics *out'),
                        self.extended().replace('int32_t execute', 'int64_t execute')):
            with self.subTest(changed=changed), self.assertRaises(RuntimeError):
                build.verify_bridge_header_extension(self.OLD, changed)

    def test_existing_ids_cannot_change_or_gain_aliases(self):
        for changed in (self.extended().replace('DLOCK_BRIDGE_FC = 1', 'DLOCK_BRIDGE_FC = 2'),
                        self.extended().replace('DLOCK_BRIDGE_SPINLOCK = 5', 'DLOCK_BRIDGE_SPINLOCK = 1'),
                        self.extended().replace('DLOCK_BRIDGE_SPINLOCK = 5', 'DLOCK_BRIDGE_FC = 5')):
            with self.subTest(changed=changed), self.assertRaises(RuntimeError):
                build.verify_bridge_header_extension(self.OLD, changed)


class FrozenDatabaseHeaders(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.old = self.root / 'frozen-headers'
        self.current = self.root / 'current-headers'
        self.old.mkdir()
        self.current.mkdir()
        for directory in (self.old, self.current):
            (directory / 'private_ops.h').write_text('int private_find(void *);\n')
        (self.old / 'bridge.h').write_text(BridgeHeaderCompatibility.OLD)
        (self.current / 'bridge.h').write_text(BridgeHeaderCompatibility().extended())
        patcher = patch.object(build, 'HERE', self.current)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.boost, self.boost_dev = self.root / 'boost', self.root / 'boost-dev'
        self.gcc = self.root / 'gcc'
        (self.gcc / 'bin').mkdir(parents=True)
        for executable in ('gcc', 'g++'):
            (self.gcc / 'bin' / executable).write_bytes(executable.encode())
        source = self.root / 'src-refactored'
        source.mkdir()
        for filename in ('configure', 'config.h.in'):
            (source / filename).write_text(filename)
        self.manifest = self.root / 'build-fc.json'
        self.recorded = {
            'schema': 2, 'pinned_sha': build.SHA, 'nixpkgs': build.NIXPKGS,
            'variant': 'fc', 'source_kind': 'refactored', 'source': str(source),
            'source_verification': {'head': build.SHA},
            'library_verification': {'shared_object_sha256': 'unchanged-database'},
            'toolchain': {
                'boost_store': str(self.boost), 'boost_dev_store': str(self.boost_dev),
                'gcc_store': str(self.gcc),
                'cc': str(self.gcc / 'bin/gcc'), 'cxx': str(self.gcc / 'bin/g++'),
                'cc_sha256': build.digest(self.gcc / 'bin/gcc'),
                'cxx_sha256': build.digest(self.gcc / 'bin/g++'),
                'cflags': '-O3 -g -DNDEBUG', 'ldflags': '-L' + str(self.boost / 'lib'),
                'cppflags': '-I' + str(self.old),
                'nix_cflags_compile_unset': True, 'nix_ldflags_unset': True,
            },
            'hashes': {
                'bridge_h_sha256': build.digest(self.old / 'bridge.h'),
                'private_ops_sha256': build.digest(self.old / 'private_ops.h'),
                'configure_sha256': build.digest(source / 'configure'),
                'config_h_in_sha256': build.digest(source / 'config.h.in'),
            },
            'build': {'harness_command': ['g++', '-I' + str(self.old)]},
        }
        self.manifest.write_text(json.dumps(self.recorded))

    def verify(self):
        return build.database_reuse_provenance(
            self.root, 'refactored', self.boost, self.boost_dev, self.gcc)

    def test_frozen_and_current_header_identities_remain_distinct(self):
        provenance, directory = self.verify()
        self.assertEqual(directory, self.old)
        self.assertEqual(provenance['build_manifest_sha256'], build.digest(self.manifest))
        self.assertEqual(provenance['compilation_header_hashes']['bridge_h_sha256'],
                         build.digest(self.old / 'bridge.h'))
        self.assertEqual(provenance['current_bridge_header_sha256'],
                         build.digest(self.current / 'bridge.h'))
        self.assertNotEqual(provenance['compilation_header_hashes']['bridge_h_sha256'],
                            provenance['current_bridge_header_sha256'])

    def test_changed_frozen_header_cannot_be_relabelled_as_current(self):
        (self.old / 'bridge.h').write_text((self.current / 'bridge.h').read_text())
        with self.assertRaises(RuntimeError):
            self.verify()

    def test_current_private_callback_change_rejects_frozen_library(self):
        (self.current / 'private_ops.h').write_text('long private_find(void *);\n')
        with self.assertRaises(RuntimeError):
            self.verify()

    def test_changed_compiler_rejects_frozen_identity(self):
        (self.gcc / 'bin/g++').write_bytes(b'new compiler')
        with self.assertRaises(RuntimeError):
            self.verify()



if __name__ == '__main__':
    unittest.main()
