from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "verify-flasher-candidate-files.py"
SPEC = importlib.util.spec_from_file_location("verify_flasher_candidate_files", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not import {SCRIPT}")
VERIFIER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFIER)


class CandidateFileVerifierTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.version = "0.2.6"
        self.channel = "preview"
        files = {
            "VERSION": f"{self.version}\n",
            "flash-manifest.json": "{}\n",
            f"channels/{self.channel}.json": "{}\n",
            "website/index.html": "candidate\n",
        }
        for relative, contents in files.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents, encoding="utf-8")
        lines = []
        for relative in sorted(files):
            digest = hashlib.sha256((self.root / relative).read_bytes()).hexdigest()
            lines.append(f"{digest}  {relative}")
        (self.root / "SHA256SUMS.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
        signatures = {
            "SHA256SUMS.txt.minisig": "checksums signature\n",
            "flash-manifest.json.minisig": "manifest signature\n",
            f"channels/{self.channel}.json.minisig": "channel signature\n",
            f"website/releases/{self.version}/flash-manifest.json.minisig": "manifest signature\n",
            f"website/releases/channels/{self.channel}.json.minisig": "channel signature\n",
        }
        for relative, contents in signatures.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents, encoding="utf-8")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_complete_signed_inventory_passes(self) -> None:
        VERIFIER.verify(self.root)

    def test_tampered_payload_is_rejected(self) -> None:
        (self.root / "website/index.html").write_text("tampered\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            VERIFIER.verify(self.root)

    def test_unlisted_payload_is_rejected(self) -> None:
        (self.root / "unexpected.txt").write_text("not signed\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "unexpected=.*unexpected.txt"):
            VERIFIER.verify(self.root)

    def test_duplicate_and_traversing_checksum_paths_are_rejected(self) -> None:
        sums = self.root / "SHA256SUMS.txt"
        original = sums.read_text(encoding="utf-8")
        first = original.splitlines()[0]
        sums.write_text(original + first + "\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "duplicate checksum path"):
            VERIFIER.verify(self.root)
        sums.write_text(f"{'0' * 64}  ../escape\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "unsafe checksum path"):
            VERIFIER.verify(self.root)

    def test_different_hosted_signature_is_rejected(self) -> None:
        hosted = self.root / "website" / "releases" / self.version / "flash-manifest.json.minisig"
        hosted.write_text("different signature\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "hosted manifest signature differs"):
            VERIFIER.verify(self.root)

    def test_historical_signature_must_be_bound_by_checksummed_history(self) -> None:
        historical = (
            self.root / "website" / "releases" / "0.2.5" / "flash-manifest.json.minisig"
        )
        historical.parent.mkdir(parents=True)
        historical.write_text("prior signature\n", encoding="utf-8")
        metadata = self.root / "metadata" / "release-history.json"
        metadata.parent.mkdir()
        metadata.write_text(
            json.dumps(
                {
                    "files": [
                        {
                            "path": "0.2.5/flash-manifest.json.minisig",
                            "size": historical.stat().st_size,
                            "sha256": hashlib.sha256(historical.read_bytes()).hexdigest(),
                        }
                    ]
                }
            )
            + "\n",
            encoding="utf-8",
        )
        sums = self.root / "SHA256SUMS.txt"
        sums.write_text(
            sums.read_text(encoding="utf-8")
            + f"{hashlib.sha256(metadata.read_bytes()).hexdigest()}  metadata/release-history.json\n",
            encoding="utf-8",
        )
        VERIFIER.verify(self.root)

        historical.write_text("tampered\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "historical signature differs"):
            VERIFIER.verify(self.root)

    def test_version_cannot_escape_the_candidate(self) -> None:
        version = self.root / "VERSION"
        version.write_text("../../outside\n", encoding="utf-8")
        lines = (self.root / "SHA256SUMS.txt").read_text(encoding="utf-8").splitlines()
        replacement = hashlib.sha256(version.read_bytes()).hexdigest() + "  VERSION"
        (self.root / "SHA256SUMS.txt").write_text(
            "\n".join(replacement if line.endswith("  VERSION") else line for line in lines)
            + "\n",
            encoding="utf-8",
        )
        with self.assertRaisesRegex(ValueError, "not an immutable release identity"):
            VERIFIER.verify(self.root)


if __name__ == "__main__":
    unittest.main()
