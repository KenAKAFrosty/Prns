import argparse
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import generate


class ApplicationOutputTests(unittest.TestCase):
    def test_symlink_is_rejected_before_host_generation_can_write_through_it(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        (self.root / 'generated').mkdir()
        outside = self.root / 'handwritten.txt'
        outside.write_text('keep\n')
        (self.root / 'generated/host-adapter.generated.ts').symlink_to(outside)
        with patch.object(generate, 'ownership', return_value=([self.root / 'generated'], [])), \
                patch.object(generate, 'run') as command:
            with self.assertRaisesRegex(ValueError, 'symlink'):
                generate.generate('cli', {}, False)
        command.assert_not_called()
        self.assertEqual(outside.read_text(), 'keep\n')


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


class StagingBoundaryTests(unittest.TestCase):
    def test_bootstrap_checks_tracked_bindings_and_writes_only_app_sdk_stage(self):
        for check, stage_only in ((False, True), (True, False)):
            with self.subTest(check=check), \
                    patch.object(generate, 'existing_outputs'), \
                    patch.object(generate, 'generated_files', return_value={}), \
                    patch.object(generate, 'synchronize_outputs') as app_outputs, \
                    patch.object(generate.sdk, 'generate') as sdk_outputs:
                generate.generate('cli', {}, check, stage_only=stage_only)
            self.assertTrue(app_outputs.call_args.args[1])
            self.assertEqual(sdk_outputs.call_args.args[-1], generate.SDK_PACKAGE)
            self.assertEqual(sdk_outputs.call_args.args[-2], check)
            self.assertTrue(generate.SDK_PACKAGE.is_relative_to(generate.APPLICATIONS / 'target'))

    def test_bootstrap_does_not_mask_stale_tracked_bindings(self):
        with patch.object(generate, 'existing_outputs'), \
                patch.object(generate, 'generated_files', return_value={}), \
                patch.object(generate, 'synchronize_outputs', side_effect=ValueError('stale app binding')), \
                patch.object(generate.sdk, 'generate') as sdk_outputs:
            with self.assertRaisesRegex(ValueError, 'stale app binding'):
                generate.execute('cli', {}, argparse.Namespace(command='stage'))
        sdk_outputs.assert_not_called()

    def test_native_builds_use_the_app_owned_sdk_destination(self):
        with patch.object(generate.sdk, 'build') as build:
            generate.build('cli', {}, argparse.Namespace(command='android', release=False))
        self.assertEqual(build.call_args.args[-1], generate.SDK_PACKAGE)
