from __future__ import annotations

from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "release"))

from flasher_public_review import (  # noqa: E402
    build_evidence,
    discover_evidence,
    evidence_asset_name,
    validate_evidence,
)


class FlasherPublicReviewTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.bundle = self.root / "prns-flasher-candidate-v0.2.6-signed.tar.gz"
        self.manifest = self.root / "flash-manifest.json"
        self.bundle.write_bytes(b"signed candidate")
        self.manifest.write_bytes(b"manifest")
        self.repository = "example/Prns"
        self.version = "0.2.6"
        self.commit = "a" * 40
        self.published = datetime(2026, 7, 20, 12, tzinfo=timezone.utc)
        self.started = self.published + timedelta(hours=24)
        self.completed = self.started + timedelta(minutes=1)
        self.release = {
            "isDraft": False,
            "isPrerelease": True,
            "tagName": f"v{self.version}",
            "targetCommitish": self.commit,
            "publishedAt": self.published.isoformat().replace("+00:00", "Z"),
        }
        self.run = {
            "id": 77,
            "run_attempt": 2,
            "path": ".github/workflows/flasher-sign.yml",
            "event": "workflow_dispatch",
            "head_sha": self.commit,
            "repository": {"full_name": self.repository},
            "status": "in_progress",
            "conclusion": None,
        }
        self.job = {
            "id": 88,
            "run_id": 77,
            "run_attempt": 2,
            "name": "Complete protected 24-hour public review",
            "head_sha": self.commit,
            "started_at": self.started.isoformat().replace("+00:00", "Z"),
            "completed_at": None,
            "status": "in_progress",
            "conclusion": None,
        }

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def build(self) -> dict:
        return build_evidence(
            release=self.release,
            run=self.run,
            job=self.job,
            signed_bundle=self.bundle,
            manifest=self.manifest,
            repository=self.repository,
            version=self.version,
            source_commit=self.commit,
            completed_at=self.completed.isoformat().replace("+00:00", "Z"),
        )

    def test_successful_exact_signing_job_is_bound(self) -> None:
        evidence = self.build()
        self.assertEqual(
            evidence["signed_candidate_sha256"],
            hashlib.sha256(self.bundle.read_bytes()).hexdigest(),
        )
        self.run.update(status="completed", conclusion="success")
        self.job.update(
            status="completed",
            conclusion="success",
            completed_at=(self.completed + timedelta(seconds=5))
            .isoformat()
            .replace("+00:00", "Z"),
        )
        validate_evidence(
            evidence,
            release=self.release,
            run=self.run,
            job=self.job,
            signed_bundle=self.bundle,
            manifest=self.manifest,
            repository=self.repository,
            version=self.version,
            source_commit=self.commit,
        )

    def test_review_cannot_start_before_publication_plus_24_hours(self) -> None:
        self.job["started_at"] = (
            self.published + timedelta(hours=23, minutes=59)
        ).isoformat().replace("+00:00", "Z")
        with self.assertRaisesRegex(ValueError, "shorter than 24 hours"):
            self.build()

    def test_wrong_workflow_or_prerelease_state_is_rejected(self) -> None:
        self.run["path"] = ".github/workflows/other.yml"
        with self.assertRaisesRegex(ValueError, "signing workflow"):
            self.build()
        self.run["path"] = ".github/workflows/flasher-sign.yml"
        self.release["isDraft"] = True
        with self.assertRaisesRegex(ValueError, "visible non-draft"):
            self.build()

    def test_validation_requires_success_and_exact_bytes(self) -> None:
        evidence = self.build()
        self.run.update(status="completed", conclusion="success")
        self.job.update(
            status="completed",
            conclusion="failure",
            completed_at=(self.completed + timedelta(seconds=5))
            .isoformat()
            .replace("+00:00", "Z"),
        )
        with self.assertRaisesRegex(ValueError, "did not complete successfully"):
            validate_evidence(
                evidence,
                release=self.release,
                run=self.run,
                job=self.job,
                signed_bundle=self.bundle,
                manifest=self.manifest,
                repository=self.repository,
                version=self.version,
                source_commit=self.commit,
            )
        self.job["conclusion"] = "success"
        self.bundle.write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "differs from the exact signed prerelease"):
            validate_evidence(
                evidence,
                release=self.release,
                run=self.run,
                job=self.job,
                signed_bundle=self.bundle,
                manifest=self.manifest,
                repository=self.repository,
                version=self.version,
                source_commit=self.commit,
            )

    def test_promoted_release_can_reverify_original_successful_attempt(self) -> None:
        evidence = self.build()
        self.release["isPrerelease"] = False
        self.run.update(status="completed", conclusion="success")
        self.job.update(
            status="completed",
            conclusion="success",
            completed_at=(self.completed + timedelta(seconds=5))
            .isoformat()
            .replace("+00:00", "Z"),
        )
        with self.assertRaisesRegex(ValueError, "visible non-draft prerelease"):
            validate_evidence(
                evidence,
                release=self.release,
                run=self.run,
                job=self.job,
                signed_bundle=self.bundle,
                manifest=self.manifest,
                repository=self.repository,
                version=self.version,
                source_commit=self.commit,
            )
        validate_evidence(
            evidence,
            release=self.release,
            run=self.run,
            job=self.job,
            signed_bundle=self.bundle,
            manifest=self.manifest,
            repository=self.repository,
            version=self.version,
            source_commit=self.commit,
            allow_promoted=True,
        )

    def test_later_run_attempt_cannot_replace_the_recorded_attempt(self) -> None:
        evidence = self.build()
        self.run.update(run_attempt=3, status="completed", conclusion="success")
        self.job.update(
            run_attempt=3,
            status="completed",
            conclusion="success",
            completed_at=(self.completed + timedelta(seconds=5))
            .isoformat()
            .replace("+00:00", "Z"),
        )
        with self.assertRaisesRegex(ValueError, "run identity differs"):
            validate_evidence(
                evidence,
                release=self.release,
                run=self.run,
                job=self.job,
                signed_bundle=self.bundle,
                manifest=self.manifest,
                repository=self.repository,
                version=self.version,
                source_commit=self.commit,
            )

    def test_persistent_attempt_assets_survive_later_reruns(self) -> None:
        evidence_directory = self.root / "public-review-assets"
        evidence_directory.mkdir()
        paths = []
        for attempt in (5, 2):
            self.run["run_attempt"] = attempt
            self.job["run_attempt"] = attempt
            self.job["id"] = 80 + attempt
            evidence = self.build()
            path = evidence_directory / evidence_asset_name(
                version=self.version,
                run_id=self.run["id"],
                run_attempt=attempt,
            )
            path.write_text(
                json.dumps(evidence, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            paths.append(path)

        discovered = discover_evidence(
            evidence_directory,
            repository=self.repository,
            version=self.version,
            source_commit=self.commit,
            workflow_run_id=self.run["id"],
            signed_candidate_sha256=hashlib.sha256(self.bundle.read_bytes()).hexdigest(),
            manifest_sha256=hashlib.sha256(self.manifest.read_bytes()).hexdigest(),
        )
        self.assertEqual(discovered, list(reversed(paths)))

    def test_persistent_asset_name_must_bind_its_attempt(self) -> None:
        evidence_directory = self.root / "public-review-assets"
        evidence_directory.mkdir()
        evidence = self.build()
        wrong_name = evidence_directory / evidence_asset_name(
            version=self.version,
            run_id=self.run["id"],
            run_attempt=self.run["run_attempt"] + 1,
        )
        wrong_name.write_text(
            json.dumps(evidence) + "\n", encoding="utf-8"
        )
        with self.assertRaisesRegex(ValueError, "must be named"):
            discover_evidence(
                evidence_directory,
                repository=self.repository,
                version=self.version,
                source_commit=self.commit,
                workflow_run_id=self.run["id"],
                signed_candidate_sha256=hashlib.sha256(
                    self.bundle.read_bytes()
                ).hexdigest(),
                manifest_sha256=hashlib.sha256(
                    self.manifest.read_bytes()
                ).hexdigest(),
            )


if __name__ == "__main__":
    unittest.main()
