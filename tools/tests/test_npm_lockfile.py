import base64
import copy
import unittest

from validation.security.npm_lockfile import verify_development_packages


class NpmBundleProvenanceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.parent = "node_modules/@vendor/archive"
        self.child = f"{self.parent}/node_modules/@vendor/child"
        self.packages = {
            self.parent: {
                "version": "1.0.0",
                "dev": True,
                "license": "MIT",
                "resolved": "https://registry.npmjs.org/@vendor/archive/-/archive-1.0.0.tgz",
                "integrity": "sha512-" + base64.b64encode(b"x" * 64).decode(),
                "bundleDependencies": ["@vendor/child"],
            },
            self.child: {
                "version": "1.0.0",
                "dev": True,
                "license": "0BSD",
                "inBundle": True,
            },
        }

    def test_declared_bundle_inherits_archive_integrity_but_keeps_its_license(self) -> None:
        verify_development_packages(self.packages, {"MIT", "0BSD"})
        with self.assertRaisesRegex(ValueError, "license"):
            verify_development_packages(self.packages, {"MIT"})

    def test_bundle_cannot_hide_missing_or_untrusted_parent_provenance(self) -> None:
        changes = (
            {"resolved": "https://untrusted.example/archive.tgz"},
            {"integrity": ""},
            {"integrity": "sha512-invalid"},
            {"bundleDependencies": []},
            {"bundleDependencies": "@vendor/child"},
            {"dev": False},
            {"inBundle": True},
        )
        for change in changes:
            with self.subTest(change=change):
                packages = copy.deepcopy(self.packages)
                packages[self.parent].update(change)
                with self.assertRaises(ValueError):
                    verify_development_packages(packages, {"MIT", "0BSD"})
        del self.packages[self.parent]
        with self.assertRaises(ValueError):
            verify_development_packages(self.packages, {"MIT", "0BSD"})

    def test_unbundled_package_must_have_its_own_integrity(self) -> None:
        del self.packages[self.child]["inBundle"]
        with self.assertRaisesRegex(ValueError, "integrity"):
            verify_development_packages(self.packages, {"MIT", "0BSD"})


if __name__ == "__main__":
    unittest.main()
