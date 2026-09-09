import io
import json
from pathlib import Path
import plistlib
import tarfile
import tempfile
import unittest

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


if __name__ == "__main__":
    unittest.main()
