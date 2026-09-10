import io
import json
import contextlib
import os
from pathlib import Path
import plistlib
import shutil
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import vendor


class PackageVerification(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "runtime.tgz"
        self.files = {
            "package.json": json.dumps({"name": "@ubjs/react-native", "version": "0.31.0-5"}).encode(),
            "LICENSE": b"upstream license notice",
            "PRNS-UPSTREAM.json": vendor.LOCK.read_bytes(),
            "cpp/shim.cpp": b"shim",
            "cpp/callbacks.cpp": b"callbacks",
            "typescript/dist/index.js": b"module",
            "UbjsReactNative.podspec": b"podspec",
            **{name: plistlib.dumps({"CFBundleShortVersionString": "0.31.0"}) if name.endswith("Info.plist") else b"native artifact" for name in vendor.REQUIRED_NATIVE},
        }

    def pack(self, extra=None):
        with tarfile.open(self.path, "w:gz") as archive:
            for name, data in self.files.items():
                entry = tarfile.TarInfo("package/" + name)
                entry.size = len(data)
                archive.addfile(entry, io.BytesIO(data))
            if extra is not None:
                archive.addfile(extra)

    def test_complete_package(self):
        self.pack()
        self.assertEqual(vendor.inspect_package(self.path, "@ubjs/react-native")["version"], "0.31.0-5")

    def test_missing_android_slice_is_rejected(self):
        del self.files[vendor.REQUIRED_NATIVE[1]]
        self.pack()
        with self.assertRaisesRegex(ValueError, "missing.*x86_64"):
            vendor.inspect_package(self.path, "@ubjs/react-native")

    def test_binary_workstation_paths_are_rejected(self):
        self.files[vendor.REQUIRED_NATIVE[0]] = b"ELF\x00/Users/user/.cargo/src\x00"
        self.pack()
        with self.assertRaisesRegex(ValueError, "workstation path"):
            vendor.inspect_package(self.path, "@ubjs/react-native")

    def test_different_source_record_is_rejected(self):
        self.files["PRNS-UPSTREAM.json"] = b"{}"
        self.pack()
        with self.assertRaisesRegex(ValueError, "source provenance"):
            vendor.inspect_package(self.path, "@ubjs/react-native")

    def test_npm_prerelease_framework_version_is_rejected(self):
        self.files[vendor.REQUIRED_NATIVE[-1]] = plistlib.dumps({"CFBundleShortVersionString": "0.31.0-5"})
        self.pack()
        with self.assertRaisesRegex(ValueError, "framework release version must be numeric"):
            vendor.inspect_package(self.path, "@ubjs/react-native")

    def test_escaping_path_is_rejected(self):
        self.pack(tarfile.TarInfo("package/../../outside"))
        with self.assertRaisesRegex(ValueError, "unsafe package path"):
            vendor.inspect_package(self.path, "@ubjs/react-native")

    def test_symlinks_are_rejected(self):
        entry = tarfile.TarInfo("package/linked-library")
        entry.type = tarfile.SYMTYPE
        entry.linkname = "../../workstation/library"
        self.pack(entry)
        with self.assertRaisesRegex(ValueError, "non-file package member"):
            vendor.inspect_package(self.path, "@ubjs/react-native")


class ReceiptVerification(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name) / "vendor"
        shutil.copytree(vendor.VENDOR, self.root)
        for name, path in (("VENDOR", self.root), ("LOCK", self.root / "source-lock.json"),
                           ("RECEIPT", self.root / "receipt.json")):
            self.enterContext(patch.object(vendor, name, path))
        self.receipt = json.loads(vendor.RECEIPT.read_text())
        self.receipt.update(schemaVersion=2, verificationScriptSha256=vendor.digest(Path(vendor.__file__)))

    def check(self):
        vendor.RECEIPT.write_text(json.dumps(self.receipt))
        with contextlib.redirect_stdout(io.StringIO()):
            vendor.check()

    def test_current_verifier_can_check_bytes_built_by_the_historical_recipe(self):
        self.receipt['buildScriptSha256'] = '1' * 64
        original = {entry['file']: (self.root / entry['file']).read_bytes()
                    for entry in self.receipt['packages']}
        self.check()
        self.assertEqual(original, {name: (self.root / name).read_bytes() for name in original})

    def test_missing_or_tampered_verifier_hash_is_rejected(self):
        for value in (None, '0' * 64):
            with self.subTest(hash=value):
                if value is None:
                    self.receipt.pop('verificationScriptSha256', None)
                else:
                    self.receipt['verificationScriptSha256'] = value
                with self.assertRaisesRegex(ValueError, 'verifier changed'):
                    self.check()

    def test_changed_archive_is_still_rejected(self):
        path = self.root / self.receipt['packages'][0]['file']
        path.write_bytes(path.read_bytes() + b'tampered')
        with self.assertRaisesRegex(ValueError, 'package digest mismatch'):
            self.check()

    def test_changed_source_is_still_rejected(self):
        self.receipt['sourceLockSha256'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'source inputs changed'):
            self.check()


class BuildToolchainSelection(unittest.TestCase):
    def test_selected_rustup_binaries_win_over_system_cargo(self):
        lock = vendor.inputs()
        expected = lock['toolchain']
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            ndk = cache / 'ndk'
            ndk.mkdir()
            (ndk / 'source.properties').write_text(f'Pkg.Revision = {expected["androidNdk"]}\n')

            def run(*command, env=None, capture=False):
                if command[:2] == ('rustup', 'which'):
                    return '/selected/rust/bin/rustc'
                if command[:2] in (('cargo', 'ndk'), ('rustc', '--version')):
                    self.assertEqual(env['PATH'].split(os.pathsep)[0], '/selected/rust/bin')
                    self.assertEqual(env['RUSTUP_TOOLCHAIN'], 'recorded')
                results = {('node', '--version'): 'v' + expected['node'],
                           ('npm', '--version'): expected['npm'],
                           ('cargo', 'ndk', '--version'): 'cargo-ndk ' + expected['cargoNdk'],
                           ('rustc', '--version'): 'rustc ' + expected['rust'] + ' (test)',
                           ('xcodebuild', '-version'): f'Xcode {expected["xcode"]}\nBuild version {expected["xcodeBuild"]}'}
                if command[:2] == ('xcrun', '--find'):
                    return '/xcode/bin/' + command[-1]
                return results[command]

            with patch.dict(os.environ, {'PATH': '/system/bin', 'ANDROID_NDK_HOME': str(ndk)}, clear=True), \
                    patch.object(vendor, 'run', side_effect=run):
                environment = vendor.build_environment(cache, lock, 'recorded')
            self.assertLess(environment['PATH'].split(os.pathsep).index('/selected/rust/bin'),
                            environment['PATH'].split(os.pathsep).index('/system/bin'))

    def test_edited_recipe_is_not_labeled_with_an_unrelated_git_commit(self):
        with patch.object(vendor.subprocess, 'check_output', side_effect=['1' * 40, b'different source']):
            recipe = vendor.build_recipe_provenance()
        self.assertNotIn('buildScriptRevision', recipe)
        self.assertEqual(recipe['buildScriptSha256'], vendor.digest(Path(vendor.__file__)))


if __name__ == "__main__":
    unittest.main()
