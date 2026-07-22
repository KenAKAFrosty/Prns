from __future__ import annotations

import hashlib
import importlib.util
from pathlib import Path
import tarfile
import tempfile
import unittest


SCRIPT = (
    Path(__file__).resolve().parents[1]
    / "release"
    / "package-flasher-qualification-evidence.py"
)
SPEC = importlib.util.spec_from_file_location("package_flasher_qualification_evidence", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not import {SCRIPT}")
PACKAGER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGER)


class QualificationEvidencePackagerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.evidence = self.root / "evidence"
        self.evidence.mkdir()
        for content in (b"first reviewed object", b"second reviewed object"):
            digest = hashlib.sha256(content).hexdigest()
            (self.evidence / digest).write_bytes(content)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_archive_is_deterministic_flat_and_link_free(self) -> None:
        first = self.root / "first.tar.gz"
        second = self.root / "second.tar.gz"
        PACKAGER.package(self.evidence, first)
        PACKAGER.package(self.evidence, second)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        with tarfile.open(first, "r:gz") as archive:
            members = archive.getmembers()
        self.assertEqual(
            [member.name for member in members], sorted(path.name for path in self.evidence.iterdir())
        )
        self.assertTrue(all(member.isfile() and not member.issym() and not member.islnk() for member in members))

    def test_misnamed_or_empty_object_is_rejected(self) -> None:
        (self.evidence / "not-a-digest").write_bytes(b"content")
        with self.assertRaisesRegex(ValueError, "named by lowercase SHA-256"):
            PACKAGER.package(self.evidence, self.root / "bad.tar.gz")

    def test_existing_archive_is_never_overwritten(self) -> None:
        output = self.root / "evidence.tar.gz"
        output.write_bytes(b"preserve")
        with self.assertRaisesRegex(ValueError, "refusing to overwrite"):
            PACKAGER.package(self.evidence, output)
        self.assertEqual(output.read_bytes(), b"preserve")


if __name__ == "__main__":
    unittest.main()
