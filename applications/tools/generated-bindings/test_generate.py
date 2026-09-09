import argparse
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import generate


class OutputOwnershipTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.manifest = self.root / 'outputs.json'
        self.document = {'schemaVersion': 1, 'directories': ['generated'],
                         'files': ['adapter.rs'], 'scaffolding': ['package.json']}
        self.manifest.write_text(json.dumps(self.document))
        self.expected = {self.root / 'generated/api.ts': 'current\n',
                         self.root / 'adapter.rs': 'adapter\n'}
        self.synchronize(check=False)

    def synchronize(self, check):
        generate.synchronize_outputs(self.expected, check, self.root, self.manifest)

    def test_check_rejects_obsolete_outputs_without_rewriting_any_file(self):
        obsolete = self.root / 'generated/old-api.ts'
        obsolete.write_text('obsolete\n')
        (self.root / 'generated/api.ts').write_text('stale\n')
        before = {file: file.read_bytes() for file in self.root.rglob('*') if file.is_file()}
        with self.assertRaisesRegex(ValueError, r'old-api.ts \(obsolete\)'):
            self.synchronize(check=True)
        self.assertEqual(before, {file: file.read_bytes() for file in before})

    def test_generation_removes_obsolete_owned_output_and_preserves_scaffolding(self):
        obsolete = self.root / 'generated/old-api.ts'
        obsolete.write_text('obsolete\n')
        scaffolding = self.root / 'package.json'
        scaffolding.write_text('owned by application\n')
        (self.root / 'generated/api.ts').write_text('stale\n')
        self.synchronize(check=False)
        self.assertFalse(obsolete.exists())
        self.assertEqual(scaffolding.read_text(), 'owned by application\n')
        self.assertEqual((self.root / 'generated/api.ts').read_text(), 'current\n')
        self.synchronize(check=True)

    def test_unowned_output_is_rejected_before_deleting_obsolete_files(self):
        obsolete = self.root / 'generated/old-api.ts'
        obsolete.write_text('obsolete\n')
        self.expected[self.root / 'package.json'] = 'would overwrite scaffolding\n'
        with self.assertRaisesRegex(ValueError, 'unowned path'):
            self.synchronize(check=False)
        self.assertTrue(obsolete.exists())
        self.assertFalse((self.root / 'package.json').exists())

    def test_symlink_in_owned_directory_never_deletes_or_overwrites_its_target(self):
        outside = self.root / 'handwritten.txt'
        outside.write_text('keep\n')
        (self.root / 'generated/old-api.ts').symlink_to(outside)
        with self.assertRaisesRegex(ValueError, 'symlink'):
            self.synchronize(check=False)
        self.assertEqual(outside.read_text(), 'keep\n')

    def test_symlink_is_rejected_before_host_generation_can_write_through_it(self):
        outside = self.root / 'handwritten.txt'
        outside.write_text('keep\n')
        (self.root / 'generated/host-adapter.generated.ts').symlink_to(outside)
        with patch.object(generate, 'ownership', return_value=([self.root / 'generated'], [])), \
                patch.object(generate, 'run') as command:
            with self.assertRaisesRegex(ValueError, 'symlink'):
                generate.generate('cli', {}, False)
        command.assert_not_called()
        self.assertEqual(outside.read_text(), 'keep\n')

    def test_manifest_rejects_path_escape_and_symlinked_directory(self):
        for invalid in ('../outside', '/absolute', '.', 'generated/../outside'):
            with self.subTest(path=invalid):
                self.document['directories'] = [invalid]
                self.manifest.write_text(json.dumps(self.document))
                with self.assertRaisesRegex(ValueError, 'invalid generated-output path'):
                    self.synchronize(check=False)
        (self.root / 'linked').symlink_to(self.root / 'generated', target_is_directory=True)
        self.document['directories'] = ['linked']
        self.manifest.write_text(json.dumps(self.document))
        with self.assertRaisesRegex(ValueError, 'symlink'):
            self.synchronize(check=False)

    def test_incomplete_generator_result_cannot_erase_a_directory(self):
        self.expected.pop(self.root / 'generated/api.ts')
        with self.assertRaisesRegex(ValueError, 'omitted an owned output directory'):
            self.synchronize(check=False)
        self.assertEqual((self.root / 'generated/api.ts').read_text(), 'current\n')


class BuildBoundaryTests(unittest.TestCase):
    def test_sdk_barrel_exposes_record_and_enum_values_without_execution_or_transport(self):
        source = '''export type Contact = { alias?: string };
export const Contact = {};
export enum ContactOutcome_Tags { Saved = "Saved" }
export const ContactOutcome = {};
export const HostSnapshotTransport = {};
export function nativeStart() {}
export async function createContact() {}
'''
        barrel = generate.api_values(source, 'pub struct HostSnapshotTransport {\n}\n')
        self.assertIn('  Contact,\n', barrel)
        self.assertIn('  ContactOutcome,\n', barrel)
        self.assertIn('  ContactOutcome_Tags,\n', barrel)
        for name in ('nativeStart', 'createContact', 'HostSnapshotTransport'):
            self.assertNotIn(name, barrel)

    def test_upstream_compiler_policy_is_isolated_without_weakening_app_policy(self):
        app = {'PATH': '/toolchain/bin', 'RUSTFLAGS': '-D warnings --cfg aes_armv8',
               'CARGO_ENCODED_RUSTFLAGS': '-D\x1fwarnings',
               'CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS': '-D warnings',
               'CARGO_BUILD_RUSTFLAGS': '-D warnings', 'CARGO_BUILD_JOBS': '4'}
        original = dict(app)
        upstream = generate.generator_environment(Path('/cache'), app)
        self.assertEqual(app, original)
        self.assertFalse(any('RUSTFLAGS' in key for key in upstream))
        self.assertEqual(upstream['CARGO_BUILD_JOBS'], '4')

    def test_mobile_image_cannot_build_when_binding_generation_fails(self):
        for platform in ('ios', 'android'):
            with self.subTest(platform=platform), \
                    patch.object(generate, 'generate', side_effect=ValueError('schema mismatch')), \
                    patch.object(generate, 'build') as build:
                with self.assertRaisesRegex(ValueError, 'schema mismatch'):
                    generate.execute('cli', {}, argparse.Namespace(command=platform))
                build.assert_not_called()

    def test_both_mobile_builds_consume_the_refreshed_sources(self):
        for platform in ('ios', 'android'):
            with self.subTest(platform=platform):
                source = {'content': 'stale'}

                def regenerate(_cli, _env, check):
                    self.assertFalse(check)
                    source['content'] = 'current'

                def build(_cli, _env, _args):
                    self.assertEqual(source['content'], 'current')

                with patch.object(generate, 'generate', side_effect=regenerate), \
                        patch.object(generate, 'build', side_effect=build) as image:
                    generate.execute('cli', {}, argparse.Namespace(command=platform))
                image.assert_called_once()


if __name__ == '__main__':
    unittest.main()
