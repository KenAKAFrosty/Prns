import json
from pathlib import Path
import tempfile
import unittest

import package_staging


class PackageStagingTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.source = self.root / 'sdk'
        self.source.mkdir()
        self.destination = self.root / 'consumer/sdk'
        self.write('package.json', json.dumps({
            'name': 'personal-rns-expo', 'version': '1.2.3',
            'files': ['src', 'ios/*.swift', 'ios/*.xcframework/**', 'android/src/main'],
            'peerDependencies': {'personal-rns': '1.2.3'},
            'devDependencies': {'personal-rns': 'file:../core'},
            'scripts': {'generate': 'python3 tools/generate.py generate'},
        }))
        self.write('outputs.json', json.dumps({'schemaVersion': 1,
            'directories': ['src/generated'], 'files': ['ios/NativeImage.generated.swift']}))
        self.write('src/index.ts', 'maintained source\n')
        self.write('src/generated/bindings.ts', 'default image generated bindings\n')
        self.write('ios/NativeImage.generated.swift', 'default_image\n')
        self.write('ios/Helper.swift', 'shared helper\n')
        self.write('ios/default.xcframework/Info.plist', 'default image\n')
        self.write('android/src/main/jniLibs/arm64-v8a/libdefault.so', 'default image\n')

    def write(self, relative, content):
        path = self.source / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def test_copies_runtime_sources_without_default_bindings_binaries_or_development_paths(self):
        package_staging.synchronize(self.source, self.destination, False)
        self.assertEqual((self.destination / 'src/index.ts').read_text(), 'maintained source\n')
        self.assertEqual((self.destination / 'ios/Helper.swift').read_text(), 'shared helper\n')
        self.assertFalse((self.destination / 'src/generated').exists())
        self.assertFalse((self.destination / 'ios/default.xcframework').exists())
        self.assertFalse((self.destination / 'android/src/main/jniLibs').exists())
        package = json.loads((self.destination / 'package.json').read_text())
        self.assertEqual(package['peerDependencies'], {'personal-rns': '1.2.3'})
        self.assertNotIn('devDependencies', package)
        self.assertNotIn('scripts', package)
        package_staging.synchronize(self.source, self.destination, True)

    def test_check_never_bootstraps_or_repairs_stale_staging(self):
        with self.assertRaisesRegex(ValueError, 'staging differs'):
            package_staging.synchronize(self.source, self.destination, True)
        self.assertFalse(self.destination.exists())
        package_staging.synchronize(self.source, self.destination, False)
        staged = self.destination / 'src/index.ts'
        staged.write_text('modified\n')
        with self.assertRaisesRegex(ValueError, 'staging differs'):
            package_staging.synchronize(self.source, self.destination, True)
        self.assertEqual(staged.read_text(), 'modified\n')

    def test_refresh_removes_obsolete_sources_and_retains_built_native_artifacts(self):
        package_staging.synchronize(self.source, self.destination, False)
        image = self.destination / 'ios/aggregate.xcframework/binary'
        image.parent.mkdir(parents=True)
        image.write_text('built aggregate\n')
        (self.source / 'ios/Helper.swift').unlink()
        with self.assertRaisesRegex(ValueError, 'staging differs'):
            package_staging.synchronize(self.source, self.destination, True)
        package_staging.synchronize(self.source, self.destination, False)
        self.assertFalse((self.destination / 'ios/Helper.swift').exists())
        self.assertEqual(image.read_text(), 'built aggregate\n')

    def test_rejects_overlapping_unowned_and_symlinked_destinations(self):
        for destination in (self.source / 'nested', self.source.parent):
            with self.subTest(destination=destination), self.assertRaisesRegex(ValueError, 'overlap'):
                package_staging.synchronize(self.source, destination, False)
        self.destination.mkdir(parents=True)
        (self.destination / 'keep').write_text('unrelated\n')
        with self.assertRaisesRegex(ValueError, 'not an owned staging package'):
            package_staging.synchronize(self.source, self.destination, False)
        symlink = self.root / 'link'
        symlink.symlink_to(self.destination, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'symlink'):
            package_staging.synchronize(self.source, symlink, False)

    def test_rejects_symlinks_in_staged_source_before_writing(self):
        package_staging.synchronize(self.source, self.destination, False)
        staged = self.destination / 'src/index.ts'
        staged.unlink()
        staged.symlink_to(self.source / 'src/index.ts')
        with self.assertRaisesRegex(ValueError, 'symlink'):
            package_staging.synchronize(self.source, self.destination, False)
        self.assertEqual((self.source / 'src/index.ts').read_text(), 'maintained source\n')

    def test_check_rejects_unexpected_publishable_sources_in_stage(self):
        package_staging.synchronize(self.source, self.destination, False)
        extra = self.destination / 'src/obsolete.ts'
        extra.write_text('old implementation\n')
        with self.assertRaisesRegex(ValueError, 'staging differs'):
            package_staging.synchronize(self.source, self.destination, True)
        package_staging.synchronize(self.source, self.destination, False)
        self.assertFalse(extra.exists())
