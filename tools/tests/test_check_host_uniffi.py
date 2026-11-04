from contextlib import redirect_stderr
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('host_uniffi_check', ROOT / 'tools/repo/check-host-uniffi.py')
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)
sys.path.insert(0, str(ROOT / 'tools/uniffi'))
import tooling


def metadata(connections=False):
    names = ['prns-host-uniffi-image', 'prns-host-uniffi', 'prns-host-runtime']
    if connections:
        names += ['prns-storage', 'redb']
    return {
        'packages': [{'id': name, 'name': name, 'manifest_path': '/repo/' + name + '/Cargo.toml'}
                     for name in names],
        'resolve': {'nodes': [
            {'id': name, 'features': ['connections'] if connections and index < 3 else [],
             'deps': [{'pkg': names[index + 1]}] if index + 1 < len(names) else []}
            for index, name in enumerate(names)]},
    }


def add_dependency(document, name, *, selected=True, manifest=None):
    document['packages'].append({'id': name, 'name': name,
                                'manifest_path': manifest or '/repo/' + name + '/Cargo.toml'})
    document['resolve']['nodes'].append({'id': name, 'features': [], 'deps': []})
    if selected:
        document['resolve']['nodes'][0]['deps'].append({'pkg': name})


class StandaloneHostCheckerTests(unittest.TestCase):
    def check_metadata(self, document, connections=False):
        with patch.object(tooling, 'run', return_value=json.dumps(document)) as run:
            CHECK.check_dependencies({'TEST': 'retained'}, connections)
        return run.call_args

    def test_shipping_profiles_select_only_connections_without_generator(self):
        for selected in (False, True):
            with self.subTest(connections=selected):
                call = self.check_metadata(metadata(selected), selected)
                command = call.args
                self.assertIn('--locked', command)
                self.assertIn('--filter-platform', command)
                self.assertIn(CHECK.IMAGE / 'Cargo.toml', command)
                self.assertEqual('--features' in command, selected)
                if selected:
                    self.assertEqual(command[command.index('--features') + 1], 'connections')
                self.assertNotIn('uniffi-bindgen', command)
                self.assertNotIn('--all-features', command)

    def test_only_selected_dependency_closure_is_evaluated(self):
        document = metadata()
        add_dependency(document, 'prns-identities', selected=False)
        self.check_metadata(document)
        document['resolve']['nodes'][0]['deps'].append({'pkg': 'prns-identities'})
        with self.assertRaisesRegex(ValueError, 'prns-identities'):
            self.check_metadata(document)

    def test_unrelated_domain_and_generation_dependencies_fail_closed(self):
        for name in ('prns-service-runtime', 'prns-service-runtime-uniffi-image', 'prns-groups',
                     'prns-identities-core', 'prns-lxmf-service', 'prns-contact-card',
                     'uniffi_bindgen', 'uniffi_bindgen_react_native'):
            with self.subTest(package=name):
                document = metadata(True)
                add_dependency(document, name)
                with self.assertRaisesRegex(ValueError, name):
                    self.check_metadata(document, True)
        document = metadata(True)
        add_dependency(document, 'product', manifest='/repo/applications/product/Cargo.toml')
        with self.assertRaisesRegex(ValueError, 'application code'):
            self.check_metadata(document, True)

    def test_missing_selection_or_implicit_storage_is_rejected(self):
        document = metadata(True)
        document['resolve']['nodes'][2]['features'] = []
        with self.assertRaisesRegex(ValueError, 'prns-host-runtime connection selection'):
            self.check_metadata(document, True)
        document = metadata()
        add_dependency(document, 'prns-storage')
        with self.assertRaisesRegex(ValueError, 'prns-storage selection'):
            self.check_metadata(document)
        document = metadata(True)
        document['resolve']['nodes'][-2]['deps'] = []
        with self.assertRaisesRegex(ValueError, 'redb selection'):
            self.check_metadata(document, True)

    def test_selected_cli_requires_actual_swift_execution_before_any_build(self):
        with patch.object(CHECK, 'run') as run, redirect_stderr(io.StringIO()) as error:
            with self.assertRaises(SystemExit) as failure:
                CHECK.main(['--connections'])
        self.assertEqual(failure.exception.code, 2)
        self.assertIn('--connections requires --language swift', error.getvalue())
        run.assert_not_called()

    def test_swift_cli_propagates_selection_and_preserves_default(self):
        for selected in (False, True):
            with self.subTest(connections=selected), tempfile.TemporaryDirectory() as directory:
                args = ['--language', 'swift'] + (['--connections'] if selected else [])
                with patch.object(CHECK.sys, 'platform', 'darwin'), \
                        patch.dict(os.environ, {'CARGO_TARGET_DIR': directory}), \
                        patch.object(CHECK, 'run') as run, \
                        patch.object(CHECK, 'qualify_swift') as qualify:
                    CHECK.main(args)
                qualify.assert_called_once_with(Path(directory), connections=selected)
                commands = [call.args for call in run.call_args_list]
                self.assertEqual(len(commands), 2)
                self.assertIn('--check', commands[0])
                self.assertEqual('--features' in commands[1], selected)
                if selected:
                    self.assertEqual(commands[1][-2:], ('--features', 'connections'))

    def test_connections_fixture_uses_host_namespace_without_composed_owner(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = CHECK.swift_fixture(Path(directory), True)
            source = fixture.read_text()
            self.assertIn('import PrnsHostBindings', source)
            self.assertIn('openHost(config:', source)
            self.assertIn('ConnectionProfiles.createNew', source)
            self.assertIn('ConnectionProfiles.openExisting', source)
            self.assertIn('prepareConnectionsConformance(path:', source)
            self.assertIn('finishConnectionsConformance(profiles:', source)
            for unrelated in ('ServiceRuntime', 'PrnsServiceRuntimeImage', 'IdentityCatalog', 'GroupClient'):
                self.assertNotIn(unrelated, source)
            self.assertEqual(CHECK.swift_fixture(Path(directory), False),
                             CHECK.IMAGE.parent / 'tests/swift_conformance.swift')

    def test_selected_recipe_checks_shipping_before_enabling_bindgen(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            events = []
            def check(env, connections=False):
                events.append(('shipping', connections))
            def generate(recipe, cli, env):
                events.append(('generation', recipe.features))
                self.assertIsNone(cli)
                self.assertEqual(recipe.manifest, CHECK.IMAGE / 'Cargo.toml')
                return {}
            with patch.object(tooling, 'environment', return_value={}), \
                    patch.object(CHECK, 'check_dependencies', side_effect=check), \
                    patch.object(tooling, 'generated_files', side_effect=generate), \
                    patch.object(tooling, 'compile_swift_consumer', return_value=target / 'consumer') as compile, \
                    patch.object(CHECK, 'run') as execute:
                CHECK.qualify_swift(target, connections=True)
            self.assertEqual(events, [('shipping', False), ('shipping', True),
                                      ('generation', ('uniffi-bindgen', 'connections'))])
            self.assertEqual(compile.call_args.kwargs['module_name'], 'PrnsHostBindings')
            self.assertEqual(compile.call_args.kwargs['native_library'],
                             target / 'debug/libprns_host_mobile.dylib')
            self.assertEqual(execute.call_args.args, (target / 'consumer',))


if __name__ == '__main__':
    unittest.main()
