import base64
import copy
import hashlib
import io
import json
from pathlib import Path
import shutil
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import react_native
import runtime_notices


def package_bytes(metadata):
    contents = io.BytesIO()
    with tarfile.open(fileobj=contents, mode='w:gz') as archive:
        data = json.dumps(metadata).encode()
        member = tarfile.TarInfo('package/package.json')
        member.size = len(data)
        archive.addfile(member, io.BytesIO(data))
    return contents.getvalue()


class PackageIntegrityTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.cache = Path(temporary.name)
        self.metadata = {'name': '@ubjs/core', 'version': '0.31.0-6', 'license': 'MPL-2.0'}
        self.contents = package_bytes(self.metadata)
        self.package = dict(self.metadata, archive='https://registry.npmjs.org/example.tgz',
                            sha256=hashlib.sha256(self.contents).hexdigest(),
                            integrity='sha512-' + base64.b64encode(
                                hashlib.sha512(self.contents).digest()).decode())

    def test_registry_bytes_and_metadata_are_verified_without_requiring_bundled_license(self):
        with patch.object(react_native.urllib.request, 'urlopen',
                          return_value=io.BytesIO(self.contents)) as download:
            archive = react_native.package_archive(self.package, self.cache)
            self.assertEqual(archive.read_bytes(), self.contents)
            self.assertEqual(react_native.package_archive(self.package, self.cache), archive)
        download.assert_called_once_with(self.package['archive'], timeout=60)

    def test_bad_download_is_rejected_before_it_enters_the_cache(self):
        with patch.object(react_native.urllib.request, 'urlopen', return_value=io.BytesIO(b'changed')):
            with self.assertRaisesRegex(ValueError, 'integrity mismatch'):
                react_native.package_archive(self.package, self.cache)
        self.assertEqual(list((self.cache / 'npm').iterdir()), [])

    def test_cache_tampering_is_rejected_without_fetching_replacement_bytes(self):
        directory = self.cache / 'npm'
        directory.mkdir()
        (directory / (self.package['sha256'] + '.tgz')).write_bytes(self.contents + b'changed')
        with patch.object(react_native.urllib.request, 'urlopen') as download:
            with self.assertRaisesRegex(ValueError, 'integrity mismatch'):
                react_native.package_archive(self.package, self.cache)
        download.assert_not_called()

    def test_valid_archive_integrity_does_not_authorize_other_metadata(self):
        for field, value in (('name', '@other/core'), ('version', '0.31.0-7'), ('license', 'MIT')):
            with self.subTest(field=field):
                package = dict(self.package, **{field: value})
                with patch.object(react_native.urllib.request, 'urlopen',
                                  return_value=io.BytesIO(self.contents)):
                    with self.assertRaisesRegex(ValueError, 'identity mismatch'):
                        react_native.package_archive(package, self.cache)

    def test_both_archive_hash_and_npm_integrity_must_match(self):
        for field in ('sha256', 'integrity'):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'integrity mismatch'):
                react_native.verify_archive_bytes(self.contents, dict(self.package, **{field: 'changed'}))


class ConfiguredInputsTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.here = self.root / 'tools/uniffi'
        self.here.mkdir(parents=True)
        self.document = json.loads(react_native.LOCK.read_text())
        for relative in {item['file'] for item in self.document['patches']} | {
                item['license_file'] for item in self.document['packages']}:
            destination = self.here / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(react_native.HERE / relative, destination)
        runtime = self.document['runtime_patch']['file']
        destination = self.root / runtime
        destination.parent.mkdir(parents=True)
        shutil.copyfile(react_native.ROOT / runtime, destination)
        self.lock = self.here / 'react-native.json'
        self.save()
        self.enterContext(patch.multiple(react_native, ROOT=self.root, HERE=self.here, LOCK=self.lock))
        self.enterContext(patch.object(runtime_notices, 'ROOT', self.root))
        (self.root / 'deny.toml').write_text(
            '[licenses]\nallow = ["MIT"]\n[[licenses.exceptions]]\n'
            'name = "one-rust-crate"\nallow = ["MPL-2.0"]\n')

    def save(self):
        self.lock.write_text(json.dumps(self.document))

    def replace_generator_patch(self, text):
        item = self.document['patches'][0]
        path = self.here / item['file']
        path.write_text(text)
        item['sha256'] = react_native.digest(path)
        self.save()

    def test_current_configuration_checks_all_maintained_inputs(self):
        self.assertEqual(react_native.inputs(), self.document)

    def test_generator_identity_ignores_runtime_attribution_and_build_tools(self):
        original = react_native.generator_id(self.document)
        changed = copy.deepcopy(self.document)
        changed['runtime_patch']['sha256'] = 'different'
        changed['toolchain']['rust'] = 'different'
        changed['packages'][0]['version'] = 'different'
        self.assertEqual(react_native.generator_id(changed), original)
        changed['patches'][0]['sha256'] = 'different'
        self.assertNotEqual(react_native.generator_id(changed), original)

    def test_modified_generator_runtime_and_license_inputs_are_rejected(self):
        cases = ((self.here / self.document['patches'][0]['file'], 'generator patch'),
                 (self.root / self.document['runtime_patch']['file'], 'runtime patch'),
                 (self.here / self.document['packages'][0]['license_file'], 'license'))
        for path, message in cases:
            original = path.read_bytes()
            with self.subTest(path=path):
                path.write_bytes(original + b'changed')
                with self.assertRaisesRegex(ValueError, message):
                    react_native.inputs()
                path.write_bytes(original)

    def test_generator_scope_checks_old_paths_and_deletions(self):
        allowed = 'crates/ubrn_bindgen/src/bindings/gen_typescript/api_module/builders.rs'
        for text in (
            f'diff --git a/runtimes/jsi/cpp/shim.cpp b/{allowed}\n',
            f'diff --git a/{allowed} b/{allowed}\n'
            '--- a/runtimes/jsi/cpp/shim.cpp\n+++ b/runtimes/jsi/cpp/shim.cpp\n',
            f'diff --git a/{allowed} b/{allowed}\n'
            'diff --git a/runtimes/jsi/cpp/shim.cpp b/runtimes/jsi/cpp/shim.cpp\n'
            'deleted file mode 100644\n--- a/runtimes/jsi/cpp/shim.cpp\n+++ /dev/null\n',
        ):
            with self.subTest(text=text):
                self.replace_generator_patch(text)
                with self.assertRaisesRegex(ValueError, 'TypeScript emitter'):
                    react_native.inputs()

    def test_runtime_patch_cannot_replace_native_binaries(self):
        item = self.document['runtime_patch']
        path = self.root / item['file']
        name = 'node_modules/@ubjs/react-native/prebuilt/android/arm64-v8a/library.so'
        path.write_text(f'diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n')
        item['sha256'] = react_native.digest(path)
        self.save()
        with self.assertRaisesRegex(ValueError, 'native source and framework metadata'):
            react_native.inputs()

    def test_binary_and_escaping_patch_inputs_are_rejected(self):
        for text in ('', 'diff --git a/file b/file\nGIT binary patch\n',
                     'diff --git a/../file b/file\n'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                react_native.patch_paths(text)

    def test_attribution_preserves_packages_patches_and_unresolved_policy(self):
        with patch.object(react_native, 'package_archive') as archives:
            report = runtime_notices.evidence()
        self.assertEqual(archives.call_count, len(self.document['packages']))
        self.assertEqual(report['source'], self.document)
        self.assertEqual(report['source_patches'][0]['sha256'], self.document['runtime_patch']['sha256'])
        for expected, actual in zip(self.document['packages'], report['packages']):
            self.assertEqual(actual['integrity'], expected['integrity'])
            self.assertEqual(actual['archive'], expected['archive'])
            self.assertEqual(actual['upstream'], self.document['upstream'])
            self.assertIn('Mozilla Public License', ' '.join(actual['license_notice'].split()))
        with self.assertRaisesRegex(ValueError, 'crate-specific exceptions do not approve'):
            runtime_notices.verify_policy(report)
        self.assertIn(self.root / self.document['runtime_patch']['file'], runtime_notices.input_paths())

    def test_failed_archive_verification_cannot_produce_attribution(self):
        with patch.object(react_native, 'package_archive', side_effect=ValueError('invalid archive')):
            with self.assertRaisesRegex(ValueError, 'invalid archive'):
                runtime_notices.evidence()


if __name__ == '__main__':
    unittest.main()
