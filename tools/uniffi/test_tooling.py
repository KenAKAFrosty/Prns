import argparse
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import tooling as generate


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

            def command(*args, cwd=None, env=None):
                calls.append((args, cwd))
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
                    (destination / 'example/host/host.kt').write_text('kotlin\n')

            with patch.object(generate, 'run', side_effect=command):
                files = generate.generated_files(recipe, 'generator', {'CARGO_TARGET_DIR': str(target)})
            build, js, foreign = (entry[0] for entry in calls)
            self.assertEqual(build[build.index('--manifest-path') + 1], recipe.manifest)
            self.assertEqual(build[build.index('-p') + 1], 'independent-host')
            self.assertEqual(js[js.index('--lib-name') + 1], 'selected_image')
            self.assertEqual(js[js.index('--library') + 1], foreign[foreign.index('--library') + 1])
            self.assertEqual(files, {
                recipe.typescript_dir / 'host.ts': 'host\n',
                recipe.typescript_dir / 'extension.ts': 'extension\n',
                recipe.swift_dir / 'host.swift': 'swift\n',
                recipe.kotlin_dir / 'example/host/host.kt': 'kotlin\n',
            })
            self.assertFalse((root / 'generated').exists())

    def test_mobile_build_rejects_wrong_ndk_before_invoking_generator(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'source.properties').write_text('Pkg.Revision = incorrect\n')
            args = argparse.Namespace(command='android', release=False, targets=None)
            with patch.object(generate, 'run') as command:
                with self.assertRaisesRegex(ValueError, 'Android bindings require NDK'):
                    generate.build_mobile('generator', {'ANDROID_NDK_HOME': str(root)}, args,
                                          root / 'ubrn.config.yaml')
            command.assert_not_called()
