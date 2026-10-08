from pathlib import Path
import json
import subprocess
import tempfile
import unittest
from unittest import mock

import working_tree
from test_run import mobility


class WorkingTreeTests(unittest.TestCase):
    def test_tilde_versions_are_not_filesystem_paths(self):
        self.assertIsNone(mobility.npm_local_path('~57.0.24'))
        self.assertEqual(mobility.npm_local_path('~/local'), '~/local')

    def test_sdk_exception_is_exact_and_identity_checked(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app, sdk = root / 'applications', root / 'prns-react-native'
            app.mkdir(); sdk.mkdir()
            (sdk / 'package.json').write_text('{"name":"personal-rns-expo"}')
            self.assertTrue(mobility.shared_sdk_package(sdk, app, 'personal-rns-expo'))
            self.assertFalse(mobility.shared_sdk_package(sdk, app, 'unrelated'))
            self.assertFalse(mobility.shared_sdk_package(sdk / 'src', app))
            (sdk / 'package.json').write_text('{"name":"unreviewed"}')
            self.assertFalse(mobility.shared_sdk_package(sdk, app))

    def test_recorded_metadata_must_exist_in_actual_pinned_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(mobility.QualificationFailure, 'recorded Prns revision lacks SDK metadata'):
                mobility.stage_sdk_metadata(root / 'recorded', root / 'export')

    def test_current_snapshot_keeps_dirty_and_new_source_but_excludes_builds(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / 'repository'; root.mkdir()
            (root / 'applications').mkdir()
            old = root / 'applications/source.txt'; old.write_text('old')
            subprocess.run(['git', 'init', '--quiet', root], check=True)
            subprocess.run(['git', '-C', root, 'add', '.'], check=True)
            subprocess.run(['git', '-C', root, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                            'commit', '--quiet', '-m', 'fixture'], check=True)
            old.write_text('current')
            (root / 'applications/new.txt').write_text('new')
            (root / 'applications/build').mkdir()
            (root / 'applications/build/cache').write_text('excluded')
            (root / 'unrelated').mkdir(); (root / 'unrelated/private').write_text('excluded')
            work = Path(temporary) / 'output'; work.mkdir()
            exported, receipt = working_tree.snapshot(root, work, {'sourceRoots': ['applications']})
            self.assertEqual((exported / 'applications/source.txt').read_text(), 'current')
            self.assertTrue((exported / 'applications/new.txt').is_file())
            self.assertFalse((exported / 'applications/build').exists())
            self.assertFalse((exported / 'unrelated').exists())
            self.assertFalse(receipt['releaseQualified'])
            self.assertEqual(receipt['kind'], 'working-tree-source')
            self.assertEqual(len(receipt['snapshotSha256']), 64)

    def test_current_snapshot_rejects_symlinks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'applications').mkdir()
            (root / 'applications/escape').symlink_to('/etc/hosts')
            with mock.patch.object(working_tree.subprocess, 'run', return_value=subprocess.CompletedProcess([],0,b'applications/escape\0')):
                with self.assertRaisesRegex(ValueError, 'escapes|symlink'):
                    working_tree.inventory(root, {'sourceRoots': ['applications']})

    def test_cargo_optional_platform_dependency_cannot_escape_policy(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            apps = root / 'applications'; apps.mkdir()
            other = root / 'unreviewed'; other.mkdir()
            (apps / 'Cargo.toml').write_text('[package]\nname="app"\nversion="0.1.0"\n[target.x.dependencies]\nevil={path="../unreviewed",optional=true}\n')
            (other / 'Cargo.toml').write_text('[package]\nname="evil"\nversion="0.1.0"\n')
            with self.assertRaisesRegex(ValueError, 'escapes source snapshot'):
                working_tree.validate_dependencies(root, {'sourceRoots':['applications'],
                    'cargoEntrypoints':['applications/Cargo.toml'],'npmPackages':{}}, mobility)

    def test_sdk_jni_core_dependencies_use_same_recorded_git_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            app = root / 'applications'; app.mkdir()
            core = root / 'personal-rns'; core.mkdir()
            jni = root / 'prns-react-native/platform/android'; jni.mkdir(parents=True)
            (core / 'Cargo.toml').write_text('[package]\nname="personal-rns"\nversion="0.0.0"\n')
            (jni / 'Cargo.toml').write_text('[package]\nname="prns-expo-android"\nversion="0.0.0"\n[dependencies]\npersonal-rns={path="../../../personal-rns"}\n')
            (app / 'Cargo.toml').write_text('[package]\nname="app"\nversion="0.0.0"\n[dependencies]\nprns-expo-android={path="../prns-react-native/platform/android"}\npersonal-rns={path="../personal-rns"}\n')
            compatibility = {"prns": {"rustPackages": {"direct": ["personal-rns"], "source": [
                {"name": "personal-rns", "path": "personal-rns", "version": "0.0.0"}]}}}
            rewrites = mobility.cargo_rewrite_plan(app, root, compatibility)
            self.assertEqual(len(rewrites), 2)
            mobility.rewrite_cargo_dependencies(app, rewrites, 'file:///recorded', '1' * 40)
            mobility.reject_external_cargo_paths(app)
            self.assertIn('git = "file:///recorded"', (jni / 'Cargo.toml').read_text())
            self.assertIn('path="../prns-react-native/platform/android"', (app / 'Cargo.toml').read_text())

    def test_checked_policy_covers_facade_native_and_android_jni(self):
        policy = working_tree.load_policy(mobility.REPOSITORY_ROOT)
        packages = working_tree.validate_dependencies(mobility.REPOSITORY_ROOT, policy, mobility)
        for name in ('prns-host-uniffi','prns-host-native','prns-expo-android','prns-app-native'):
            self.assertIn(name, packages)

    def test_workspace_projection_preserves_inherited_policy(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'Cargo.toml').write_text('''[workspace]
resolver = "2"
members = [
    "personal-rns",
    "unrelated-firmware",
]
default-members = [
    "personal-rns",
    "unrelated-firmware",
]
[workspace.package]
version = "0.3.7"
[workspace.lints.rust]
unsafe_code = "forbid"
[profile.release]
lto = true
''')
            receipt = {}
            working_tree.project_workspace(root, {'personal-rns': 'personal-rns/Cargo.toml'}, receipt)
            result = working_tree.tomllib.loads((root / 'Cargo.toml').read_text())
            self.assertEqual(result['workspace']['members'], ['personal-rns'])
            self.assertEqual(result['workspace']['default-members'], ['personal-rns'])
            self.assertEqual(result['workspace']['lints']['rust']['unsafe_code'], 'forbid')
            self.assertTrue(result['profile']['release']['lto'])
            self.assertEqual(receipt['workspaceProjection']['omitted']['members'], ['unrelated-firmware'])
            self.assertNotEqual(receipt['workspaceProjection']['sourceSha256'], receipt['workspaceProjection']['projectedSha256'])

    def test_current_artifact_policy_selects_staged_aggregate_without_exporting_builds(self):
        policy = working_tree.load_policy(mobility.REPOSITORY_ROOT)
        self.assertEqual(policy['npmPackages']['personal-rns-expo'], 'prns-react-native')
        self.assertEqual(policy['npmArtifacts']['personal-rns-expo'], 'applications/target/react-native-sdk')
        self.assertNotIn('applications/target/react-native-sdk', policy['sourceRoots'])
