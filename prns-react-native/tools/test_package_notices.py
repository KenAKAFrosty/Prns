import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import package_notices as notices


class SelectionTests(unittest.TestCase):
    def test_cargo_owns_shipping_closure_and_dev_only_edges_are_excluded(self):
        # Cargo's target-filtered graph may contain dev packages and unrelated
        # workspace members. Neither is a shipping attribution input.
        root = notices.ROOT
        provider_path = notices.PACKAGE / 'providers/default.json'
        provider = notices.generate.image_provider(provider_path)
        def package(identity, name, manifest):
            return {'id': identity, 'name': name, 'version': '1', 'source': None,
                    'manifest_path': str(root / manifest), 'license_file': None}
        metadata = {'workspace_root': str(root), 'packages': [
            package('image', provider['package'], 'prns-host/bindings/uniffi/image/Cargo.toml'),
            package('normal', 'prns-host', 'prns-host/core/Cargo.toml'),
            package('dev', 'not-shipped', 'prns-lxmf/wire/Cargo.toml'),
            package('unrelated', 'not-selected', 'prns-identities/core/Cargo.toml')],
            'resolve': {'nodes': [
                {'id': 'image', 'deps': [
                    {'pkg': 'normal', 'dep_kinds': [{'kind': None}]},
                    {'pkg': 'dev', 'dep_kinds': [{'kind': 'dev'}]}]},
                {'id': 'normal', 'deps': []}, {'id': 'dev', 'deps': []},
                {'id': 'unrelated', 'deps': []}]}}
        commands = []
        def cargo(command, **_):
            commands.append(command)
            return type('Output', (), {'stdout': json.dumps(metadata)})()
        with patch.object(notices.subprocess, 'run', side_effect=cargo):
            inputs, packages = notices.selected_inputs(provider_path, provider, {'source': {'targets': ['target']}})
        self.assertEqual(set(packages), {'prns-host/bindings/uniffi/image/Cargo.toml', 'prns-host/core/Cargo.toml'})
        self.assertNotIn('--features', commands[0])  # no generation-only bindgen
        self.assertIn('--locked', commands[0])
        self.assertEqual(commands[0][-2:], ['--filter-platform', 'target'])
        self.assertTrue(any(row['path'] == 'Cargo.lock' for row in inputs))
        self.assertTrue(any(row['path'] == 'Cargo.toml' for row in inputs))
        # Model a changed inherited workspace license while Cargo reports the
        # same package identities. The fast input record must still change.
        original_digest = notices.digest
        def changed_workspace(path):
            return 'changed-workspace-license' if path == root / 'Cargo.toml' else original_digest(path)
        with patch.object(notices.subprocess, 'run', side_effect=cargo), \
                patch.object(notices, 'digest', side_effect=changed_workspace):
            changed, _ = notices.selected_inputs(provider_path, provider, {'source': {'targets': ['target']}})
        before = {item['path']: item['sha256'] for item in inputs}
        after = {item['path']: item['sha256'] for item in changed}
        self.assertEqual({name for name in before if before[name] != after[name]}, {'Cargo.toml'})


    def test_inputs_and_notice_bytes_are_checked_against_selected_image(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            selection = {'schemaVersion': 1, 'provider': 'sample', 'libraryName': 'sample'}
            record = {'provider': 'sample.json', 'selection': selection, 'inputs': [{'sha256': 'old'}]}
            (target / notices.NOTICE).write_text('license')
            stored = copy.deepcopy(record)
            stored['notice_sha256'] = notices.digest(target / notices.NOTICE)
            (target / notices.SOURCES).write_text(json.dumps(stored))
            (target / 'native-image.json').write_text(json.dumps(selection))
            with patch.object(notices, 'prepare', side_effect=lambda _: (None, None, copy.deepcopy(record))):
                self.assertEqual(notices.check_inputs(target), stored)
                (target / notices.NOTICE).write_text('changed')
                with self.assertRaisesRegex(ValueError, 'inputs or notice bytes changed'):
                    notices.check_inputs(target)
                (target / notices.NOTICE).write_text('license')
                (target / 'native-image.json').write_text(json.dumps({**selection, 'provider': 'other'}))
                with self.assertRaisesRegex(ValueError, 'different selected native image'):
                    notices.check_inputs(target)
                (target / notices.SOURCES).unlink()
                with self.assertRaisesRegex(ValueError, 'missing'):
                    notices.check_inputs(target)
