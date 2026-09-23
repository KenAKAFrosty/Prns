import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

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
