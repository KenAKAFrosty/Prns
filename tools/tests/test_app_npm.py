from __future__ import annotations

import base64
import copy
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock

from validation.security import app_npm

ROOT = Path(__file__).resolve().parents[2]


def registry(name, version='1.0.0', **fields):
    return dict(version=version, license='MIT',
                resolved=f'https://registry.npmjs.org/{name}/-/{name}-{version}.tgz',
                integrity='sha512-' + base64.b64encode(b'x' * 64).decode(), **fields)


class AppClosureTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.app = {
            '': {'name': 'installer', 'devDependencies': {'dev-tool': '1'}},
            'prns/app': {'name': '@prns/app', 'dependencies': {'sdk': '*', 'ordinary': '*'}},
            'prns/bindings': {'name': '@prns/app-bindings'},
            'prns/platform': {'name': '@prns/app-platform'},
            'target/react-native-sdk': {'name': 'sdk', 'peerDependencies': {'runtime': '1', 'personal-rns': '1'}},
            'node_modules/sdk': {'link': True, 'resolved': 'target/react-native-sdk'},
            'node_modules/personal-rns': {'link': True, 'resolved': '../prns-js'},
            'node_modules/runtime': registry('runtime', dev=True),
            'node_modules/ordinary': registry('ordinary'),
            'node_modules/dev-tool': registry('dev-tool', dev=True),
        }
        self.js = {
            '': {'name': 'personal-rns', 'dependencies': {'casework': '1'},
                 'optionalDependencies': {'native-platform': '1'},
                 'peerDependencies': {'framework': '1'}, 'peerDependenciesMeta': {'framework': {'optional': True}}},
            'node_modules/casework': registry('casework'),
            'node_modules/native-platform': {'optional': True},
            'node_modules/framework': registry('framework', dev=True, dependencies={'dev-tool': '*'}),
            'node_modules/dev-tool': registry('dev-tool', dev=True),
        }
        self.write()

    def write(self):
        for project, packages in [('applications', self.app), ('prns-js', self.js)]:
            (self.root / project).mkdir(exist_ok=True)
            (self.root / project / 'package.json').write_text(json.dumps(packages['']))
            (self.root / project / 'package-lock.json').write_text(json.dumps({'lockfileVersion': 3, 'packages': packages}))
        for path, source in app_npm.SOURCE_MANIFESTS.items():
            file = self.root / source
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text(json.dumps(self.app[path]))

    def test_runtime_peers_and_linked_source_deps_ignore_incorrect_dev_bits(self):
        result = app_npm.app_closure(self.root)
        self.assertEqual({p['name'] for p in result['packages']}, {'runtime', 'ordinary', 'casework'})
        self.assertEqual(result['unresolved_optional'], [['prns-js', '', 'native-platform']])
        self.assertEqual(result['unselected_optional_peers'], [['prns-js', '', 'framework']])
        self.assertFalse((self.root / 'applications/target/react-native-sdk/package.json').exists())

    def test_missing_required_peer_is_not_treated_as_optional(self):
        del self.app['node_modules/runtime']
        self.write()
        result = app_npm.app_closure(self.root)
        self.assertEqual(result['unresolved_required_peers'],
                         [['applications', 'target/react-native-sdk', 'runtime', '1']])
        self.assert_peer_audit_fails(result, 'missing required peer runtime@1')

    def assert_peer_audit_fails(self, closure, message):
        report = dict(closure, packages=[])
        with self.assertRaisesRegex(ValueError, message):
            app_npm.audit_notices(report)

    def test_missing_runtime_dependency_still_fails_collection(self):
        del self.app['node_modules/ordinary']
        self.write()
        with self.assertRaisesRegex(ValueError, 'missing required runtime dependency ordinary'):
            app_npm.app_closure(self.root)

    def test_parent_optional_peer_does_not_waive_required_drawer_peer(self):
        self.app['prns/app']['dependencies']['router'] = '*'
        self.app['node_modules/router'] = registry('router', dependencies={'drawer': '*'},
            peerDependencies={'gesture': '*'}, peerDependenciesMeta={'gesture': {'optional': True}})
        self.app['node_modules/drawer'] = registry('drawer', peerDependencies={'gesture': '>=2'})
        for direct in (False, True):
            with self.subTest(direct=direct):
                if direct:
                    self.app['prns/app']['dependencies']['drawer'] = '*'
                self.write()
                result = app_npm.app_closure(self.root)
                self.assertEqual(result['unresolved_required_peers'],
                                 [['applications', 'node_modules/drawer', 'gesture', '>=2']])
                self.assert_peer_audit_fails(result, 'missing required peer gesture@>=2')

    def test_absent_optional_peer_is_not_a_required_peer_failure(self):
        self.app['node_modules/ordinary'].update(peerDependencies={'gesture': '*'},
            peerDependenciesMeta={'gesture': {'optional': True}})
        self.write()
        result = app_npm.app_closure(self.root)
        self.assertEqual(result['unresolved_required_peers'], [])
        app_npm.audit_notices(dict(result, packages=[]))

    def test_source_sdk_declarations_cannot_drift_from_staged_lock_metadata(self):
        path = self.root / 'prns-react-native/package.json'
        changed = json.loads(path.read_text())
        changed['peerDependencies']['new-runtime'] = '1'
        path.write_text(json.dumps(changed))
        with self.assertRaisesRegex(ValueError, 'peerDependencies disagrees'):
            app_npm.app_closure(self.root)

    def test_nested_resolution_and_optional_dependency_override_are_exact(self):
        self.app['node_modules/ordinary']['dependencies'] = {'runtime': '2'}
        self.app['node_modules/ordinary/node_modules/runtime'] = registry('runtime', '2.0.0')
        self.write()
        result = app_npm.app_closure(self.root)
        self.assertEqual({p['version'] for p in result['packages'] if p['name'] == 'runtime'}, {'1.0.0', '2.0.0'})

    def test_arbitrary_link_or_missing_registry_integrity_fails_closed(self):
        self.app['node_modules/sdk']['resolved'] = '../../other/generated-sdk'
        self.write()
        with self.assertRaisesRegex(ValueError, 'unreviewed source link'):
            app_npm.app_closure(self.root)
        self.app['node_modules/sdk']['resolved'] = 'target/react-native-sdk'
        del self.app['node_modules/ordinary']['integrity']
        self.write()
        with self.assertRaisesRegex(ValueError, 'registry source/integrity'):
            app_npm.app_closure(self.root)

    def test_local_archives_cannot_replace_registry_runtime_inputs(self):
        self.app['node_modules/runtime']['resolved'] = 'file:runtime.tgz'
        self.write()
        with self.assertRaisesRegex(ValueError, 'registry source/integrity'):
            app_npm.app_closure(self.root)

    def test_installed_registry_optional_peer_is_included_even_if_marked_dev(self):
        self.app['node_modules/ordinary']['peerDependencies']={'optional-runtime':'*'}
        self.app['node_modules/ordinary']['peerDependenciesMeta']={'optional-runtime':{'optional':True}}
        self.app['node_modules/optional-runtime']=registry('optional-runtime', dev=True)
        self.write()
        self.assertIn('optional-runtime', {p['name'] for p in app_npm.app_closure(self.root)['packages']})

    def test_real_app_root_preserves_selected_optional_and_missing_required_peers(self):
        result = app_npm.app_closure(ROOT)
        names = {p['name'] for p in result['packages']}
        self.assertTrue({'casework', '@ubjs/core', '@ubjs/react-native', 'expo-camera'} <= names)
        self.assertTrue({'vite', '@builder.io/qwik', 'svelte', 'vue'}.isdisjoint(names))
        # Router's installed testing-library peer reaches its installed Jest peer.
        # Declaration reachability is deliberately broader than bundled JS.
        self.assertTrue({'@testing-library/react-native', 'jest'} <= names)
        self.assertEqual(result['unresolved_required_peers'], [
            ['applications', 'node_modules/react-native-drawer-layout', 'react-native-gesture-handler', '>= 2.0.0'],
            ['applications', 'node_modules/react-native-drawer-layout', 'react-native-reanimated', '>= 2.0.0'],
        ])
        self.assert_peer_audit_fails(result, 'missing required peer react-native-gesture-handler')


class AppArchiveAndAdvisoryTests(unittest.TestCase):
    def test_archive_integrity_is_checked_before_reading_and_on_cache_reuse(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            content = b'locked archive bytes'
            package = {'resolved': 'https://registry.npmjs.org/runtime/-/runtime-1.0.0.tgz',
                       'integrity': 'sha512-' + base64.b64encode(hashlib.sha512(content).digest()).decode()}
            with mock.patch.object(app_npm.urllib.request, 'urlopen', return_value=io.BytesIO(content)) as fetch:
                archive = app_npm.package_archive(root, package, root / 'cache')
                self.assertEqual(archive.read_bytes(), content)
                self.assertEqual(app_npm.package_archive(root, package, root / 'cache'), archive)
                self.assertEqual(fetch.call_count, 1)
            archive.write_bytes(b'different')
            with self.assertRaisesRegex(ValueError, 'differs'):
                app_npm.package_archive(root, package, root / 'cache')
            with self.assertRaisesRegex(ValueError, 'unreviewed npm archive source'):
                app_npm.package_archive(root, dict(package, resolved='file:archive.tgz'), root / 'cache')

    def test_bundled_license_and_unlicense_names_are_preserved_without_extraction(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'archive.tgz'
            with tarfile.open(path, 'w:gz') as output:
                for name, content in [('package/dist/code.js.LICENSE.txt', b'reproduced terms'),
                                      ('package/UNLICENSE', b'public domain declaration')]:
                    info = tarfile.TarInfo(name); info.size = len(content)
                    output.addfile(info, io.BytesIO(content))
            text = app_npm.package_notices(path)
            self.assertIn('reproduced terms', text)
            self.assertIn('public domain declaration', text)
            self.assertEqual(list(Path(directory).iterdir()), [path])

    def test_no_license_text_is_an_error_not_an_inferred_license(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'archive.tgz'
            with tarfile.open(path, 'w:gz'):
                pass
            with self.assertRaisesRegex(ValueError, 'no license/notice'):
                app_npm.package_notices(path)

    def test_audit_uses_exact_production_identities_and_preserves_failures(self):
        closure = {'packages': [dict(name='runtime', **registry('runtime', dev=True))]}
        seen = []
        def run(command, **kwargs):
            seen.append(command)
            lock = json.loads((kwargs['cwd'] / 'package-lock.json').read_text())
            package = lock['packages']['node_modules/audited-0']
            self.assertEqual((package['name'], package['version']), ('runtime', '1.0.0'))
            self.assertNotIn('dev', package)
            self.assertIn('--package-lock-only', command)
            self.assertIn('--ignore-scripts', command)
            self.assertNotIn('--audit-level', command)
            return subprocess.CompletedProcess(command, 1, '{"vulnerabilities":{"runtime":{}}}', '')
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(app_npm.subprocess, 'run', side_effect=run):
            output = Path(directory) / 'audit.json'
            with self.assertRaisesRegex(ValueError, 'advisory audit failed'):
                app_npm.audit_advisories(closure, output)
            self.assertIn('vulnerabilities', output.read_text())
        self.assertEqual(len(seen), 1)



class LicensePolicyTests(unittest.TestCase):
    def test_compound_spdx_uses_existing_permissions_without_new_exceptions(self):
        allowed = {'MIT', 'Apache-2.0', 'BSD-3-Clause', 'Apache-2.0 WITH LLVM-exception'}
        for expression in ('MIT OR Apache-2.0', '(MIT AND Apache-2.0)',
                           '(BSD-3-Clause OR GPL-2.0)', 'Apache-2.0 WITH LLVM-exception',
                           'MPL-2.0 AND GPL-2.0 OR MIT'):
            self.assertTrue(app_npm.license_permitted(expression, allowed), expression)
        for expression in ('MIT AND MPL-2.0', 'MPL-2.0', 'MIT; rm', 'MIT OR',
                           'MIT WITH unknown', '(MIT', 'MIT Apache-2.0'):
            self.assertFalse(app_npm.license_permitted(expression, allowed), expression)

    def test_empty_fallback_cannot_approve_an_archive_without_license_text(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            folder=root/'release/licenses/app-npm';folder.mkdir(parents=True)
            package=dict(name='missing', **registry('missing'))
            (folder/'provenance.json').write_text(json.dumps({'schema':1,'packages':[
                dict(name='missing',version=package['version'],integrity=package['integrity'],texts=[],unresolved=[])]}))
            (root/'deny.toml').write_text('[licenses]\nallow=["MIT"]\n')
            with mock.patch.object(app_npm,'app_closure',return_value={'packages':[package], 'unresolved_required_peers':[]}), \
                 mock.patch.object(app_npm,'package_archive',return_value=Path('unused')), \
                 mock.patch.object(app_npm,'package_notices',side_effect=ValueError('no license')):
                report=app_npm.notice_report(root,Path('unused'))
            with self.assertRaisesRegex(ValueError,'No verified license'):
                app_npm.audit_notices(report)

    def test_official_runtime_license_fallback_preserves_unapproved_mpl_policy(self):
        provenance = json.loads((ROOT / 'release/licenses/app-npm/provenance.json').read_text())
        records = [item for item in provenance['packages'] if item['name'] in {'@ubjs/core', '@ubjs/react-native'}]
        self.assertEqual(len(records), 2)
        packages = [dict(name=item['name'], version=item['version'], integrity=item['integrity'],
                         resolved=f'https://registry.npmjs.org/{item["name"]}/-/{item["name"].split("/")[1]}-{item["version"]}.tgz',
                         license='MPL-2.0') for item in records]
        with mock.patch.object(app_npm, 'app_closure', return_value={'packages': packages, 'unresolved_required_peers': []}), \
             mock.patch.object(app_npm, 'package_archive', return_value=Path('unused')), \
             mock.patch.object(app_npm, 'package_notices', side_effect=ValueError('no license')):
            report = app_npm.notice_report(ROOT, Path('unused'))
        for package in report['packages']:
            self.assertEqual(package['attribution_issues'], [])
            self.assertEqual(len(package['texts']), 1)
            self.assertIn('Mozilla Public', package['texts'][0]['text'])
            self.assertTrue(package['policy_issues'])
        with self.assertRaisesRegex(ValueError, 'MPL-2.0'):
            app_npm.audit_notices(report)

    def test_notice_evidence_never_turns_missing_attribution_into_approval(self):
        root = Path(__file__).resolve().parents[2]
        with mock.patch.object(app_npm, 'package_archive', return_value=Path('unused')), \
             mock.patch.object(app_npm, 'package_notices', return_value='Verified archive text'):
            report = app_npm.notice_report(root, Path('unused'))
        casework = next(p for p in report['packages'] if p['name'] == 'casework')
        self.assertTrue(casework['attribution_issues'])
        alternative = next(p for p in report['packages'] if p['name'] == 'fb-dotslash')
        self.assertFalse(alternative['policy_issues'])
        with self.assertRaisesRegex(ValueError, 'casework'):
            app_npm.audit_notices(report)

if __name__ == '__main__':
    unittest.main()
