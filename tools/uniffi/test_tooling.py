import argparse
import importlib.util
import json
from pathlib import Path
import tempfile
import sys
import unittest
from unittest.mock import patch

import tooling as generate


KOTLIN_CHECKSUM_SOURCE = """// Other generated code stays unchanged.
internal object IntegrityCheckingUniffiLib {
    external fun uniffi_example_checksum_func_example(
    ): Int
}
private fun uniffiCheckApiChecksums(lib: IntegrityCheckingUniffiLib) {
    if (lib.uniffi_example_checksum_func_example() != 34967) {
        throw RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project")
    }
}
// Contract checks and domain methods stay unchanged.
"""
KOTLIN_CHECKSUM_CORRECTED = KOTLIN_CHECKSUM_SOURCE.replace(
    'lib.uniffi_example_checksum_func_example()',
    'lib.uniffi_example_checksum_func_example().toUShort().toInt()')


class KotlinChecksumTests(unittest.TestCase):
    def test_preserves_int_carrier_expected_value_throw_and_other_source(self):
        self.assertEqual(generate.normalize_kotlin_checksums(
            KOTLIN_CHECKSUM_SOURCE, 'uniffi-bindgen 0.31.2'), KOTLIN_CHECKSUM_CORRECTED)

    def test_namespace_without_checksum_methods_keeps_empty_enforcement_block(self):
        source = 'private fun uniffiCheckApiChecksums(lib: IntegrityCheckingUniffiLib) {\n}\n'
        self.assertEqual(generate.normalize_kotlin_checksums(source, 'uniffi-bindgen 0.31.2'), source)

    def test_other_generator_version_requires_explicit_review(self):
        for version in ('uniffi-bindgen 0.31.1', 'uniffi-bindgen 0.32.1', ''):
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, 'version'):
                generate.normalize_kotlin_checksums(KOTLIN_CHECKSUM_SOURCE, version)

    def test_missing_disabled_or_changed_checks_fail_closed(self):
        for source in (
            '', KOTLIN_CHECKSUM_SOURCE + KOTLIN_CHECKSUM_SOURCE,
            KOTLIN_CHECKSUM_SOURCE.replace('): Int', '): Short'),
            KOTLIN_CHECKSUM_SOURCE.replace('34967', '65536'),
            KOTLIN_CHECKSUM_SOURCE.replace('34967', '-1'),
            KOTLIN_CHECKSUM_SOURCE.replace('!=', '=='),
            KOTLIN_CHECKSUM_SOURCE.replace('throw RuntimeException', 'println'),
            KOTLIN_CHECKSUM_SOURCE.replace('    external fun', '    // external fun'),
            KOTLIN_CHECKSUM_SOURCE.replace('    if (', '    // if ('),
            KOTLIN_CHECKSUM_CORRECTED,
        ):
            with self.subTest(source=source), self.assertRaises(ValueError):
                generate.normalize_kotlin_checksums(source, 'uniffi-bindgen 0.31.2')

    def test_empty_function_cannot_hide_non_int_checksum_declarations(self):
        source = ('    external fun uniffi_example_checksum_func_example(\n    ): Short\n'
                  'private fun uniffiCheckApiChecksums(lib: IntegrityCheckingUniffiLib) {\n}\n')
        with self.assertRaisesRegex(ValueError, 'template'):
            generate.normalize_kotlin_checksums(source, 'uniffi-bindgen 0.31.2')

    def test_extra_int_checksum_declaration_cannot_omit_its_comparison(self):
        source = KOTLIN_CHECKSUM_SOURCE.replace(
            'internal object IntegrityCheckingUniffiLib {',
            'internal object IntegrityCheckingUniffiLib {\n'
            '    external fun uniffi_example_checksum_func_other(\n    ): Int')
        with self.assertRaisesRegex(ValueError, 'template'):
            generate.normalize_kotlin_checksums(source, 'uniffi-bindgen 0.31.2')


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
        self.assertEqual(upstream['CARGO_TARGET_DIR'], str(Path('/cache/cli-target').resolve()))
        shared = generate.generator_environment(
            Path('/cache'), dict(app, PRNS_UBRN_TARGET_DIR='/shared-build'))
        self.assertEqual(shared['CARGO_TARGET_DIR'], str(Path('/shared-build').resolve()))
        self.assertFalse(any('RUSTFLAGS' in key for key in shared))
        self.assertEqual(app, original)



class IndependentRecipeTests(unittest.TestCase):
    def test_recipe_selects_its_metadata_image_and_all_output_namespaces(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            target = root / 'target'
            target.mkdir()
            recipe = generate.BindingRecipe(
                manifest=root / 'Cargo.toml', package='independent-host',
                library_name='selected_image', crate_dir=root / 'host',
                typescript_dir=root / 'generated/ts',
                swift_dir=root / 'generated/swift', kotlin_dir=root / 'generated/kotlin',
                uniffi_config=root / 'host/uniffi.toml', features=('uniffi',),
                extra_bins=('fingerprints',),
            )
            calls = []

            def command(*args, cwd=None, env=None, capture=False):
                calls.append((args, cwd))
                if args[-1] == '--version':
                    return 'uniffi-bindgen 0.31.2\n'
                if args[:2] == ('cargo', 'build'):
                    return
                if '--ts-dir' in args:
                    destination = Path(args[args.index('--ts-dir') + 1])
                    destination.mkdir()
                    (destination / 'host.ts').write_text('host  \n')
                    (destination / 'extension.ts').write_text('extension\n')
                else:
                    destination = Path(args[args.index('--out-dir') + 1])
                    (destination / 'example/host').mkdir(parents=True)
                    (destination / 'host.swift').write_text('swift\n')
                    (destination / 'example/host/host.kt').write_text(KOTLIN_CHECKSUM_SOURCE)

            with patch.object(generate, 'run', side_effect=command):
                files = generate.generated_files(recipe, 'generator', {'CARGO_TARGET_DIR': str(target)})
            build, js, version, foreign = (entry[0] for entry in calls)
            self.assertEqual(version[-1], '--version')
            self.assertEqual(build[build.index('--manifest-path') + 1], recipe.manifest)
            self.assertEqual(build[build.index('-p') + 1], 'independent-host')
            self.assertEqual(js[js.index('--lib-name') + 1], 'selected_image')
            self.assertEqual(js[js.index('--library') + 1], foreign[foreign.index('--library') + 1])
            self.assertEqual(files, {
                recipe.typescript_dir / 'host.ts': 'host\n',
                recipe.typescript_dir / 'extension.ts': 'extension\n',
                recipe.swift_dir / 'host.swift': 'swift\n',
                recipe.kotlin_dir / 'example/host/host.kt': KOTLIN_CHECKSUM_CORRECTED,
            })
            self.assertFalse((root / 'generated').exists())

    def test_native_only_recipes_do_not_invoke_or_require_rn_generator(self):
        for languages in (('swift',), ('kotlin',), ('swift', 'kotlin')):
            with self.subTest(languages=languages), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                target = root / 'target'
                target.mkdir()
                recipe = generate.BindingRecipe(
                    manifest=root / 'Cargo.toml', package='independent-host',
                    library_name='selected_image', crate_dir=root,
                    **{f'{language}_dir': root / language for language in languages},
                )
                calls = []

                def command(*args, **_kwargs):
                    calls.append(args)
                    if args[-1] == '--version':
                        return 'uniffi-bindgen 0.31.2\n'
                    if args[:2] == ('cargo', 'build'):
                        return
                    self.assertNotIn('jsi2', args)
                    selected = [args[i + 1] for i, arg in enumerate(args) if arg == '--language']
                    self.assertEqual(selected, list(languages))
                    destination = Path(args[args.index('--out-dir') + 1])
                    destination.mkdir()
                    if 'swift' in selected:
                        (destination / 'Host.swift').write_text('swift\n')
                        (destination / 'HostFFI.h').write_text('header\n')
                        (destination / 'HostFFI.modulemap').write_text('module\n')
                    if 'kotlin' in selected:
                        (destination / 'example').mkdir()
                        (destination / 'example/host.kt').write_text(KOTLIN_CHECKSUM_SOURCE)

                with patch.object(generate, 'run', side_effect=command), \
                        patch.object(generate, '_react_native_module', side_effect=AssertionError('RN helper imported')):
                    files = generate.generated_files(recipe, None, {'CARGO_TARGET_DIR': str(target)})
                self.assertEqual(len(calls), 3 if 'kotlin' in languages else 2)
                self.assertEqual({file.parts[len(root.parts)] for file in files}, set(languages))
                self.assertFalse((root / 'swift').exists())
                self.assertFalse((root / 'kotlin').exists())

    def test_isolated_native_recipe_imports_and_generates_without_react_native_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            source = root / 'tools/uniffi/tooling.py'
            source.parent.mkdir(parents=True)
            source.write_text(Path(generate.__file__).read_text())
            self.assertFalse((source.parent / 'react_native.py').exists())
            self.assertFalse((source.parent / 'react-native.json').exists())
            spec = importlib.util.spec_from_file_location('isolated_uniffi_tooling', source)
            native = importlib.util.module_from_spec(spec)
            with patch.dict(sys.modules, {spec.name: native}):
                spec.loader.exec_module(native)
                target = root / 'target'
                target.mkdir()
                recipe = native.BindingRecipe(
                    manifest=root / 'Cargo.toml', package='identity-image',
                    library_name='identity_keys', crate_dir=root,
                    swift_dir=root / 'swift', kotlin_dir=root / 'kotlin',
                    features=('uniffi-bindgen',),
                )
                calls = []

                def command(*args, **_kwargs):
                    calls.append(args)
                    if args[-1] == '--version':
                        return 'uniffi-bindgen 0.31.2\n'
                    if args[:2] == ('cargo', 'build'):
                        return
                    self.assertEqual(args[1], 'generate')
                    self.assertNotIn('jsi2', args)
                    destination = Path(args[args.index('--out-dir') + 1])
                    destination.mkdir()
                    (destination / 'Keys.swift').write_text('swift\n')
                    (destination / 'KeysFFI.h').write_text('header\n')
                    (destination / 'KeysFFI.modulemap').write_text('module\n')
                    (destination / 'identity').mkdir()
                    (destination / 'identity/keys.kt').write_text(KOTLIN_CHECKSUM_SOURCE)

                with patch.object(native, 'run', side_effect=command):
                    files = native.generated_files(recipe, None, {'CARGO_TARGET_DIR': str(target)})
                self.assertEqual(len(calls), 3)
                self.assertEqual(files, {
                    root / 'swift/Keys.swift': 'swift\n',
                    root / 'swift/KeysFFI.h': 'header\n',
                    root / 'swift/KeysFFI.modulemap': 'module\n',
                    root / 'kotlin/identity/keys.kt': KOTLIN_CHECKSUM_CORRECTED,
                })
                # Selecting RN still requires its real helper; native success
                # must not silently substitute or weaken that prerequisite.
                with self.assertRaises(FileNotFoundError):
                    native.generator_inputs()

    def test_typescript_only_recipe_does_not_build_native_cli(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            recipe = generate.BindingRecipe(
                manifest=root / 'Cargo.toml', package='independent-host',
                library_name='selected_image', crate_dir=root, typescript_dir=root / 'ts',
            )
            calls = []

            def command(*args, **_kwargs):
                calls.append(args)
                if '--ts-dir' in args:
                    destination = Path(args[args.index('--ts-dir') + 1])
                    destination.mkdir()
                    (destination / 'host.ts').write_text('typescript\n')

            with patch.object(generate, 'run', side_effect=command):
                files = generate.generated_files(recipe, 'rn-generator', {'CARGO_TARGET_DIR': str(root)})
            self.assertEqual(len(calls), 2)
            self.assertNotIn('--bin', calls[0])
            self.assertEqual(files, {root / 'ts/host.ts': 'typescript\n'})

    def test_missing_language_or_typescript_generator_fails_before_build(self):
        root = Path('/independent')
        recipe = generate.BindingRecipe(
            manifest=root / 'Cargo.toml', package='independent-host',
            library_name='selected_image', crate_dir=root,
        )
        with patch.object(generate, 'run') as command:
            with self.assertRaisesRegex(ValueError, 'at least one output language'):
                generate.generated_files(recipe, None, {})
            with self.assertRaisesRegex(ValueError, 'requires the pinned RN generator'):
                generate.generated_files(
                    generate.BindingRecipe(**{**recipe.__dict__, 'typescript_dir': root / 'ts'}),
                    None, {},
                )
            command.assert_not_called()

    def test_mobile_build_rejects_wrong_ndk_before_invoking_generator(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'source.properties').write_text('Pkg.Revision = incorrect\n')
            args = argparse.Namespace(command='android', release=False, targets=None)
            # The generic helper also ships without React Native tooling. This
            # unit test owns its input metadata instead of loading that consumer.
            with patch.object(generate, '_react_native_module') as native, \
                    patch.object(generate, 'run') as command:
                native.return_value.inputs.return_value = {'toolchain': {'androidNdk': 'expected-ndk'}}
                with self.assertRaisesRegex(ValueError, 'Android bindings require NDK'):
                    generate.build_mobile('generator', {'ANDROID_NDK_HOME': str(root)}, args,
                                          root / 'ubrn.config.yaml')
            command.assert_not_called()


class SwiftConsumerTests(unittest.TestCase):
    def test_selected_image_and_module_are_linked_without_generation_or_execution(self):
        root = Path('/independent')
        env = {'CARGO_TARGET_DIR': str(root / 'target')}
        with patch.object(generate, 'run') as command:
            executable = generate.compile_swift_consumer(
                generated_sources=(root / 'generated/Telemetry.swift',),
                module_maps=(root / 'headers/TelemetryFFI.modulemap',), module_name='Telemetry',
                native_library=root / 'native/libtelemetry.dylib',
                fixture=root / 'tests/consumer.swift', output_dir=root / 'build', env=env,
            )
        self.assertEqual(executable, root / 'build/swift-conformance')
        self.assertEqual(command.call_count, 2)
        module, consumer = command.call_args_list
        self.assertIn(root / 'native/libtelemetry.dylib', module.args)
        self.assertIn(root / 'native', module.args)
        self.assertIn(root / 'generated/Telemetry.swift', module.args)
        self.assertIn(root / 'build/libTelemetry.dylib', consumer.args)
        self.assertIn(root / 'tests/consumer.swift', consumer.args)
        for call in (module, consumer):
            self.assertEqual(call.args[:2], ('xcrun', 'swiftc'))
            self.assertEqual(call.args[call.args.index('-module-cache-path') + 1],
                             root / 'target/swift-module-cache')
            self.assertIn('-fmodule-map-file=/independent/headers/TelemetryFFI.modulemap', call.args)
            self.assertEqual(call.kwargs, {'cwd': root / 'build', 'env': env})

    def test_multiple_namespaces_share_one_module(self):
        root = Path('/independent')
        sources = (root / 'a/First.swift', root / 'b/Second.swift')
        maps = (root / 'a/FirstFFI.modulemap', root / 'b/SecondFFI.modulemap')
        with patch.object(generate, 'run') as command:
            generate.compile_swift_consumer(
                generated_sources=sources, module_maps=maps, module_name='Together',
                native_library=root / 'native/libimage.dylib',
                fixture=root / 'consumer.swift', output_dir=root / 'build', env={},
            )
        module, consumer = command.call_args_list
        for source in sources:
            self.assertIn(source, module.args)
        for call in (module, consumer):
            self.assertEqual(call.args[call.args.index('-module-cache-path') + 1],
                             root / 'build/swift-module-cache')
            for path in maps:
                self.assertIn(f'-fmodule-map-file={path}', call.args)
                self.assertIn(path.parent, call.args)
        self.assertIn(root / 'build/libTogether.dylib', consumer.args)

    def test_empty_inputs_fail_before_invoking_compiler(self):
        for sources, maps in (((), (Path('/A.modulemap'),)), ((Path('/A.swift'),), ())):
            with patch.object(generate, 'run') as command, self.assertRaises(ValueError):
                generate.compile_swift_consumer(
                    generated_sources=sources, module_maps=maps, module_name='Empty',
                    native_library=Path('/lib.dylib'), fixture=Path('/test.swift'),
                    output_dir=Path('/output'), env={},
                )
            command.assert_not_called()
