import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location('sdk_generation', Path(__file__).with_name('generate.py'))
generate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(generate)


class ImageProviderTests(unittest.TestCase):
    def test_default_provider_is_independent_of_application_tree(self):
        provider = generate.image_provider(generate.PACKAGE / 'providers/default.json')
        self.assertEqual(provider['libraryName'], 'prns_host_mobile')
        for key in ('manifest', 'crateDirectory'):
            self.assertNotIn('applications', provider[key].parts)

    def test_rejects_missing_and_duplicate_image_providers(self):
        provider = json.loads((generate.PACKAGE / 'providers/default.json').read_text())['providers'][0]
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'providers.json'
            for providers in ([], [provider, dict(provider, id='aggregate')]):
                path.write_text(json.dumps({'schemaVersion': 1, 'providers': providers}))
                with self.assertRaisesRegex(ValueError, 'exactly one native image'):
                    generate.image_provider(path)

    def test_platform_selections_agree_and_have_no_source_paths(self):
        provider = generate.image_provider(generate.PACKAGE / 'providers/default.json')
        for path, content in generate.selection_files(provider).items():
            self.assertIn('prns_host_mobile', content)
            self.assertNotIn(str(generate.ROOT), content)
            self.assertTrue(path.is_relative_to(generate.PACKAGE))

    def test_generator_features_do_not_enter_native_image_builds(self):
        default = generate.image_provider(generate.PACKAGE / 'providers/default.json')
        providers = [default, dict(default, id='aggregate', features=['host-test'])]
        for provider in providers:
            with self.subTest(provider=provider['id']), tempfile.TemporaryDirectory() as temporary:
                destination = Path(temporary).resolve()
                with patch.object(generate.tooling, 'generated_files', return_value={}) as generated, \
                        patch.object(generate.tooling, 'synchronize_outputs'), \
                        patch.object(generate.package_staging, 'synchronize'):
                    generate.generate(provider, 'generator', {}, True, destination)
                recipe = generated.call_args.args[0]
                self.assertIn('uniffi-bindgen', recipe.features)
                self.assertNotIn('uniffi-bindgen', provider['features'])
                configs = []

                def capture_config(cli, env, args, config):
                    configs.append(config.read_text())

                with patch.object(generate.tooling, 'build_mobile', side_effect=capture_config):
                    generate.build(provider, 'generator', {}, SimpleNamespace(), destination)
                self.assertEqual(len(configs), 1)
                self.assertNotIn('uniffi-bindgen', configs[0])

    def test_rejects_invalid_generation_features(self):
        provider = generate.image_provider(generate.PACKAGE / 'providers/default.json')
        provider['manifest'] = str(provider['manifest'])
        provider['crateDirectory'] = str(provider['crateDirectory'])
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'providers.json'
            for invalid in ('uniffi-bindgen', [False]):
                provider['generationFeatures'] = invalid
                path.write_text(json.dumps({'schemaVersion': 1, 'providers': [provider]}))
                with self.assertRaisesRegex(ValueError, 'generationFeatures must be a list of strings'):
                    generate.image_provider(path)

    def test_aggregate_cannot_replace_default_tracked_sdk(self):
        default = generate.image_provider(generate.PACKAGE / 'providers/default.json')
        for provider in (dict(default, id='aggregate'), dict(default, libraryName='aggregate_image')):
            with self.subTest(provider=provider), patch.object(generate.tooling, 'generated_files') as generated:
                with self.assertRaisesRegex(ValueError, 'separate --destination'):
                    generate.generate(provider, 'generator', {}, False)
            generated.assert_not_called()

    def test_destination_owns_generated_namespace_and_mobile_build(self):
        provider = generate.image_provider(generate.PACKAGE / 'providers/default.json')
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary).resolve()
            with patch.object(generate.tooling, 'generated_files', return_value={}), \
                    patch.object(generate.tooling, 'synchronize_outputs') as synchronize:
                generate.generate(provider, 'generator', {}, False, destination)
            files, check, root, _ = synchronize.call_args.args
            self.assertEqual(root, destination)
            self.assertTrue(all(path.is_relative_to(destination) for path in files))
            self.assertIn(destination / 'src/generated/contract.generated.ts', files)
            configs = []
            with patch.object(generate.tooling, 'build_mobile', side_effect=lambda _cli, _env, _args, config: configs.append(config)):
                generate.build(provider, 'generator', {}, SimpleNamespace(), destination)
            self.assertEqual(configs[0].parent, destination)

    def test_destination_cannot_reuse_binaries_from_another_image(self):
        provider = generate.image_provider(generate.PACKAGE / 'providers/default.json')
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary).resolve()
            generate.package_staging.synchronize(generate.PACKAGE, destination, False)
            (destination / 'native-image.json').write_text(json.dumps({'libraryName': 'another_image'}))
            with self.assertRaisesRegex(ValueError, 'another image'):
                generate.selected_destination(provider, destination)
