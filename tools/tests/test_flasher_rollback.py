from __future__ import annotations

from datetime import datetime, timedelta, timezone
from functools import partial
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import threading
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "tools" / "release"
sys.path.insert(0, str(SCRIPTS))

from flasher_rollback import (
    COMING_SOON,
    STABLE_RELEASE,
    create_dry_run_record,
    stage,
    stage_coming_soon,
    validate_coming_soon,
    validate_descriptor,
    validate_dry_run_record,
    validate_live_state,
    validate_promotion_state,
    verify_live_website,
)
from flasher_website_history import (
    apply_history,
    bootstrap_blocking_custody_tags,
    prepare_bootstrap,
    prepare_retained,
    sha256,
    stable_descriptor_identity,
    validate_candidate_history,
)


VERSION = "0.2.6"
NEXT_VERSION = "0.2.7"
SOURCE_COMMIT = "a" * 40
WORKFLOW_SHA = "c" * 40
REPOSITORY = "example/Prns"


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def historical_verifier_module():
    script = SCRIPTS / "verify-historical-flasher-release.py"
    spec = importlib.util.spec_from_file_location(
        "verify_historical_flasher_release", script
    )
    if spec is None or spec.loader is None:
        raise AssertionError(f"could not import {script}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def descriptor(version: str, manifest_sha256: str) -> dict:
    return {
        "schema": 1,
        "channel": "stable",
        "version": version,
        "manifest_url": f"https://reticulum.rs/releases/{version}/flash-manifest.json",
        "manifest_sha256": manifest_sha256,
    }


def bootstrap_metadata() -> dict:
    return {
        "schema": 1,
        "mode": "bootstrap",
        "head": None,
        "tree": {
            "file_count": 0,
            "total_bytes": 0,
            "tree_sha256": hashlib.sha256(b"").hexdigest(),
        },
        "files": [],
    }


def signed_candidate(
    root: Path, version: str = VERSION, *, manifest_schema: int = 3
) -> tuple[Path, Path]:
    root.mkdir(parents=True)
    manifest = {
        "schema": manifest_schema,
        "release": {
            "version": version,
            "channel": "stable",
            "commit": SOURCE_COMMIT,
        },
        "signing": {"key_id": "0123456789ABCDEF"},
        "targets": [],
    }
    manifest_path = root / "flash-manifest.json"
    write_json(manifest_path, manifest)
    website = root / "website"
    (website / "index.html").parent.mkdir(parents=True)
    (website / "index.html").write_text(f"site {version}\n", encoding="utf-8")
    release = website / "releases" / version
    release.mkdir(parents=True)
    (release / "flash-manifest.json").write_bytes(manifest_path.read_bytes())
    (release / "flash-manifest.json.minisig").write_text(
        "fixture signature\n", encoding="utf-8"
    )
    stable = descriptor(version, sha256(manifest_path))
    write_json(website / "releases" / "channels" / "stable.json", stable)
    (website / "releases" / "channels" / "stable.json.minisig").write_text(
        "fixture channel signature\n", encoding="utf-8"
    )
    write_json(root / "metadata" / "release-history.json", bootstrap_metadata())
    record = root.parent / f"flasher-release-record-v{version}.json"
    write_json(
        record,
        {
            "schema": 1,
            "release": {
                "version": version,
                "channel": "stable",
                "source_commit": SOURCE_COMMIT,
                "signing_key_id": "0123456789ABCDEF",
            },
            "candidate": {
                "archive": {
                    "name": f"prns-flasher-candidate-v{version}-signed.tar.gz",
                    "size": 100,
                    "sha256": "b" * 64,
                },
                "manifest": {
                    "sha256": sha256(manifest_path),
                    "signature_sha256": "d" * 64,
                },
            },
        },
    )
    return manifest_path, record


class FlasherRollbackTests(unittest.TestCase):
    def test_historical_hotfix_uses_hotfix_aware_asset_policy(self) -> None:
        module = historical_verifier_module()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            candidate = root / "candidate"
            historical = (
                root / "snapshot" / "tools" / "release" / "verify-flasher-release-assets.py"
            )
            candidate.mkdir()
            historical.parent.mkdir(parents=True)
            historical.write_text("historical verifier\n", encoding="utf-8")

            self.assertEqual(
                module.release_asset_verifier(candidate, root / "snapshot"),
                historical.resolve(),
            )

            write_json(candidate / "metadata" / "hotfix.json", {"schema": 1})
            self.assertEqual(
                module.release_asset_verifier(candidate, root / "snapshot"),
                (SCRIPTS / "verify-flasher-release-assets.py").resolve(),
            )

    def test_bootstrap_guard_distinguishes_suite_and_flasher_custody(self) -> None:
        signed_candidate = {"name": "prns-flasher-candidate-v0.3.0-signed.tar.gz"}
        prerelease = {
            "tag_name": "v0.3.0",
            "draft": False,
            "prerelease": True,
            "assets": [signed_candidate],
        }
        self.assertEqual(bootstrap_blocking_custody_tags([prerelease]), [])

        stable = {**prerelease, "prerelease": False}
        self.assertEqual(bootstrap_blocking_custody_tags([stable]), ["v0.3.0"])

        suite_record = {
            **prerelease,
            "assets": [{"name": "release-record-v0.3.0.json"}],
        }
        self.assertEqual(bootstrap_blocking_custody_tags([suite_record]), [])

        finalized_flasher_record = {
            **prerelease,
            "assets": [{"name": "flasher-release-record-v0.3.0.json"}],
        }
        self.assertEqual(
            bootstrap_blocking_custody_tags([finalized_flasher_record]), ["v0.3.0"]
        )

        with self.assertRaisesRegex(ValueError, "metadata is malformed"):
            bootstrap_blocking_custody_tags(
                [{"tag_name": "v0.3.0", "draft": False, "assets": []}]
            )

    def test_history_bootstrap_is_explicit_empty_and_retention_is_cumulative(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            bootstrap = workspace / "bootstrap"
            metadata = prepare_bootstrap(bootstrap)
            self.assertEqual(metadata, bootstrap_metadata())

            bootstrap_candidate = workspace / "bootstrap-candidate"
            bootstrap_candidate.mkdir()
            write_json(
                bootstrap_candidate / "flash-manifest.json",
                {
                    "schema": 3,
                    "release": {
                        "version": NEXT_VERSION,
                        "channel": "stable",
                        "commit": "e" * 40,
                    },
                },
            )
            (bootstrap_candidate / "website").mkdir()
            apply_history(bootstrap, bootstrap_candidate)
            validate_candidate_history(bootstrap_candidate)

            prior = workspace / "prior"
            _, record = signed_candidate(prior, manifest_schema=2)
            with self.assertRaisesRegex(ValueError, "candidate manifest is not schema 3"):
                validate_candidate_history(prior)
            retained = workspace / "retained"
            retained_metadata = prepare_retained(prior, record, retained)
            self.assertEqual(retained_metadata["mode"], "retained")
            self.assertEqual(retained_metadata["head"]["version"], VERSION)
            self.assertTrue(
                (retained / "releases" / VERSION / "flash-manifest.json.minisig").is_file()
            )

            current = workspace / "current"
            current.mkdir()
            write_json(
                current / "flash-manifest.json",
                {
                    "schema": 3,
                    "release": {
                        "version": NEXT_VERSION,
                        "channel": "stable",
                        "commit": "e" * 40,
                    },
                },
            )
            (current / "website").mkdir()
            apply_history(retained, current)
            current_release = current / "website" / "releases" / NEXT_VERSION
            current_release.mkdir()
            (current_release / "flash-manifest.json").write_text(
                "current\n", encoding="utf-8"
            )
            validate_candidate_history(current)

            historical = current / "website" / "releases" / VERSION / "flash-manifest.json"
            historical.write_text("tampered\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "retained bytes"):
                validate_candidate_history(current)

    def test_history_rejects_metadata_shape_drift_and_probes_live_content(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            candidate = workspace / "candidate"
            signed_candidate(candidate)
            metadata_path = candidate / "metadata" / "release-history.json"
            metadata = json.loads(metadata_path.read_text())
            metadata["unreviewed"] = True
            write_json(metadata_path, metadata)
            with self.assertRaisesRegex(ValueError, "unsupported shape"):
                validate_candidate_history(candidate)

            fallback = workspace / "stable.html"
            fallback.write_text("<!doctype html><title>coming soon</title>\n", encoding="utf-8")
            self.assertIsNone(stable_descriptor_identity(fallback))
            malformed = workspace / "stable-malformed.json"
            write_json(malformed, {"schema": 1, "channel": "stable"})
            with self.assertRaisesRegex(ValueError, "unsupported shape"):
                stable_descriptor_identity(malformed)
            canonical = workspace / "stable.json"
            write_json(canonical, descriptor(VERSION, "f" * 64))
            self.assertEqual(
                stable_descriptor_identity(canonical),
                {"version": VERSION, "manifest_sha256": "f" * 64},
            )

    def test_rollback_stage_cas_and_successful_dry_run_record(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            candidate = workspace / "candidate"
            manifest, record = signed_candidate(candidate)
            record_hash = sha256(record)
            staged = workspace / "staged"
            identity_path = workspace / "stage.json"
            identity = stage(
                candidate=candidate,
                release_record=record,
                release_record_sha256=record_hash,
                version=VERSION,
                output=staged,
                identity_output=identity_path,
            )
            self.assertEqual(identity["target"]["manifest_sha256"], sha256(manifest))
            self.assertEqual((staged / "index.html").read_text(), f"site {VERSION}\n")
            self.assertEqual(identity["schema"], 3)
            self.assertEqual(identity["target"]["kind"], STABLE_RELEASE)
            self.assertIn("files", identity["website"])

            class QuietHandler(SimpleHTTPRequestHandler):
                def log_message(self, format: str, *args: object) -> None:
                    pass

            server = ThreadingHTTPServer(
                ("127.0.0.1", 0), partial(QuietHandler, directory=str(staged))
            )
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                site_url = f"http://127.0.0.1:{server.server_port}"
                verified_website = verify_live_website(
                    stage_identity=identity_path, site_url=site_url
                )
                self.assertEqual(verified_website, identity["website"])
                (staged / "index.html").write_text("tampered\n", encoding="utf-8")
                with self.assertRaisesRegex(
                    ValueError, "deployed rollback website .* differs"
                ):
                    verify_live_website(stage_identity=identity_path, site_url=site_url)
                (staged / "index.html").write_text(
                    f"site {VERSION}\n", encoding="utf-8"
                )
            finally:
                server.shutdown()
                server.server_close()
                thread.join()

            live_descriptor = workspace / "stable.json"
            live_hash = "f" * 64

            write_json(live_descriptor, descriptor(NEXT_VERSION, live_hash))
            validate_descriptor(live_descriptor, NEXT_VERSION, live_hash)
            with self.assertRaisesRegex(ValueError, "compare-and-swap"):
                validate_descriptor(live_descriptor, VERSION, sha256(manifest))
            self.assertEqual(
                validate_live_state(
                    live_descriptor,
                    mode="dry-run",
                    target_kind=STABLE_RELEASE,
                    target_version=VERSION,
                    target_manifest_sha256=sha256(manifest),
                    expected_live_version=NEXT_VERSION,
                    expected_live_manifest_sha256=live_hash,
                ),
                "expected_live",
            )
            self.assertEqual(
                validate_live_state(
                    live_descriptor,
                    mode="deploy",
                    target_kind=STABLE_RELEASE,
                    target_version=VERSION,
                    target_manifest_sha256=sha256(manifest),
                    expected_live_version=NEXT_VERSION,
                    expected_live_manifest_sha256=live_hash,
                ),
                "expected_live",
            )
            write_json(live_descriptor, descriptor(VERSION, sha256(manifest)))
            self.assertEqual(
                validate_live_state(
                    live_descriptor,
                    mode="dry-run",
                    target_kind=STABLE_RELEASE,
                    target_version=VERSION,
                    target_manifest_sha256=sha256(manifest),
                    expected_live_version=NEXT_VERSION,
                    expected_live_manifest_sha256=live_hash,
                ),
                "target_baseline",
            )
            self.assertEqual(
                validate_live_state(
                    live_descriptor,
                    mode="deploy",
                    target_kind=STABLE_RELEASE,
                    target_version=VERSION,
                    target_manifest_sha256=sha256(manifest),
                    expected_live_version=NEXT_VERSION,
                    expected_live_manifest_sha256=live_hash,
                ),
                "target_idempotent_resume",
            )

            started = datetime(2026, 7, 21, 12, 0, tzinfo=timezone.utc)
            completed = started + timedelta(seconds=90)
            dry_record = workspace / "rollback-dry-run.json"
            create_dry_run_record(
                stage_identity=identity_path,
                expected_live_version=NEXT_VERSION,
                expected_live_manifest_sha256=live_hash,
                repository=REPOSITORY,
                workflow_run_id=77,
                workflow_run_attempt=1,
                workflow_job_id=88,
                workflow_sha=WORKFLOW_SHA,
                observed_live_state="target_baseline",
                started_epoch=int(started.timestamp()),
                output=dry_record,
                now=completed,
            )
            self.assertEqual(
                json.loads(dry_record.read_text())["deployment_cas"],
                "deferred_to_deploy",
            )
            run_json = workspace / "run.json"
            job_json = workspace / "job.json"
            write_json(
                run_json,
                {
                    "id": 77,
                    "repository": {"full_name": REPOSITORY},
                    "head_repository": {"full_name": REPOSITORY},
                    "path": ".github/workflows/flasher-rollback.yml",
                    "event": "workflow_dispatch",
                    "status": "completed",
                    "conclusion": "success",
                    "head_branch": "main",
                    "head_sha": WORKFLOW_SHA,
                    "run_attempt": 1,
                    "run_started_at": started.isoformat(),
                    "updated_at": (started + timedelta(seconds=120)).isoformat(),
                },
            )
            write_json(
                job_json,
                {
                    "id": 88,
                    "run_id": 77,
                    "name": "Verify and stage complete prior website",
                    "status": "completed",
                    "conclusion": "success",
                    "head_sha": WORKFLOW_SHA,
                    "run_attempt": 1,
                    "started_at": started.isoformat(),
                    "completed_at": (started + timedelta(seconds=120)).isoformat(),
                },
            )
            validated = validate_dry_run_record(
                record_path=dry_record,
                run_json=run_json,
                job_json=job_json,
                stage_identity=identity_path,
                repository=REPOSITORY,
                default_branch="main",
                expected_run_id=77,
                expected_run_attempt=1,
                target_kind=STABLE_RELEASE,
                target_version=VERSION,
                target_release_record_sha256=record_hash,
                expected_live_version=NEXT_VERSION,
                expected_live_manifest_sha256=live_hash,
                required_workflow_sha=WORKFLOW_SHA,
                required_observed_live_state="target_baseline",
            )
            self.assertEqual(validated["elapsed_seconds"], 90)
            with self.assertRaisesRegex(ValueError, "wrong workflow run attempt"):
                validate_dry_run_record(
                    record_path=dry_record,
                    run_json=run_json,
                    job_json=job_json,
                    stage_identity=identity_path,
                    repository=REPOSITORY,
                    default_branch="main",
                    expected_run_id=77,
                    expected_run_attempt=2,
                    target_kind=STABLE_RELEASE,
                    target_version=VERSION,
                    target_release_record_sha256=record_hash,
                    expected_live_version=NEXT_VERSION,
                    expected_live_manifest_sha256=live_hash,
                    required_workflow_sha=WORKFLOW_SHA,
                )
            with self.assertRaisesRegex(ValueError, "wrong live release state"):
                validate_dry_run_record(
                    record_path=dry_record,
                    run_json=run_json,
                    job_json=job_json,
                    stage_identity=identity_path,
                    repository=REPOSITORY,
                    default_branch="main",
                    expected_run_id=77,
                    expected_run_attempt=1,
                    target_kind=STABLE_RELEASE,
                    target_version=VERSION,
                    target_release_record_sha256=record_hash,
                    expected_live_version=NEXT_VERSION,
                    expected_live_manifest_sha256=live_hash,
                    required_workflow_sha=WORKFLOW_SHA,
                    required_observed_live_state="expected_live",
                )

            job = json.loads(job_json.read_text())
            job["completed_at"] = (started + timedelta(seconds=901)).isoformat()
            write_json(job_json, job)
            run = json.loads(run_json.read_text())
            run["updated_at"] = job["completed_at"]
            write_json(run_json, run)
            with self.assertRaisesRegex(ValueError, "exceeded 15 minutes"):
                validate_dry_run_record(
                    record_path=dry_record,
                    run_json=run_json,
                    job_json=job_json,
                    stage_identity=identity_path,
                    repository=REPOSITORY,
                    default_branch="main",
                    expected_run_id=77,
                    expected_run_attempt=1,
                    target_kind=STABLE_RELEASE,
                    target_version=VERSION,
                    target_release_record_sha256=record_hash,
                    expected_live_version=NEXT_VERSION,
                    expected_live_manifest_sha256=live_hash,
                    required_workflow_sha=WORKFLOW_SHA,
                    required_observed_live_state="target_baseline",
                )

    def test_rollback_stage_accepts_verified_historical_schema_only(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            historical = workspace / "historical"
            manifest, record = signed_candidate(historical, manifest_schema=2)
            identity = stage(
                candidate=historical,
                release_record=record,
                release_record_sha256=sha256(record),
                version=VERSION,
                output=workspace / "historical-staged",
                identity_output=workspace / "historical-stage.json",
            )
            self.assertEqual(identity["target"]["manifest_sha256"], sha256(manifest))

            future = workspace / "future"
            _, future_record = signed_candidate(future, manifest_schema=4)
            with self.assertRaisesRegex(
                ValueError, "rollback target is not the exact signed stable candidate"
            ):
                stage(
                    candidate=future,
                    release_record=future_record,
                    release_record_sha256=sha256(future_record),
                    version=VERSION,
                    output=workspace / "future-staged",
                    identity_output=workspace / "future-stage.json",
                )

    def test_coming_soon_target_is_exact_and_binds_withdrawn_release(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            repository = workspace / "repository"
            for relative in (
                "docs/website/coming-soon/index.html",
                "docs/website/public/CNAME",
                "docs/website/public/assets/favicon.svg",
                "docs/website/public/assets/prns-mark.svg",
                "docs/website/public/assets/og.png",
            ):
                path = repository / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(f"{relative}\n", encoding="utf-8")
            staged = workspace / "coming-soon"
            identity_path = workspace / "coming-soon-stage.json"
            manifest_sha256 = "f" * 64
            identity = stage_coming_soon(
                repository=repository,
                withdrawn_version=NEXT_VERSION,
                withdrawn_manifest_sha256=manifest_sha256,
                output=staged,
                identity_output=identity_path,
            )
            self.assertEqual(
                identity["target"],
                {
                    "kind": COMING_SOON,
                    "withdrawn_version": NEXT_VERSION,
                    "withdrawn_manifest_sha256": manifest_sha256,
                },
            )
            self.assertEqual(
                (staged / "404.html").read_bytes(),
                (staged / "index.html").read_bytes(),
            )
            live = workspace / "stable-response"
            live.write_bytes((staged / "index.html").read_bytes())
            validate_coming_soon(
                live,
                repository / "docs/website/coming-soon/index.html",
            )
            self.assertEqual(
                validate_live_state(
                    live,
                    mode="dry-run",
                    target_kind=COMING_SOON,
                    target_version=None,
                    target_manifest_sha256=None,
                    expected_live_version=NEXT_VERSION,
                    expected_live_manifest_sha256=manifest_sha256,
                    coming_soon_index=(
                        repository / "docs/website/coming-soon/index.html"
                    ),
                ),
                "target_baseline",
            )
            self.assertEqual(
                validate_promotion_state(
                    live,
                    baseline_kind=COMING_SOON,
                    baseline_version=None,
                    baseline_manifest_sha256=None,
                    candidate_version=NEXT_VERSION,
                    candidate_manifest_sha256=manifest_sha256,
                    coming_soon_index=(
                        repository / "docs/website/coming-soon/index.html"
                    ),
                ),
                "baseline",
            )
            write_json(live, descriptor(NEXT_VERSION, manifest_sha256))
            self.assertEqual(
                validate_promotion_state(
                    live,
                    baseline_kind=COMING_SOON,
                    baseline_version=None,
                    baseline_manifest_sha256=None,
                    candidate_version=NEXT_VERSION,
                    candidate_manifest_sha256=manifest_sha256,
                    coming_soon_index=(
                        repository / "docs/website/coming-soon/index.html"
                    ),
                ),
                "candidate_idempotent_resume",
            )
            started = datetime(2026, 7, 21, 12, 0, tzinfo=timezone.utc)
            completed = started + timedelta(seconds=30)
            record_path = workspace / "coming-soon-dry-run.json"
            create_dry_run_record(
                stage_identity=identity_path,
                expected_live_version=NEXT_VERSION,
                expected_live_manifest_sha256=manifest_sha256,
                repository=REPOSITORY,
                workflow_run_id=77,
                workflow_run_attempt=1,
                workflow_job_id=88,
                workflow_sha=WORKFLOW_SHA,
                observed_live_state="target_baseline",
                started_epoch=int(started.timestamp()),
                output=record_path,
                now=completed,
            )
            run_json = workspace / "coming-soon-run.json"
            job_json = workspace / "coming-soon-job.json"
            write_json(
                run_json,
                {
                    "id": 77,
                    "repository": {"full_name": REPOSITORY},
                    "head_repository": {"full_name": REPOSITORY},
                    "path": ".github/workflows/flasher-rollback.yml",
                    "event": "workflow_dispatch",
                    "status": "completed",
                    "conclusion": "success",
                    "head_branch": "main",
                    "head_sha": WORKFLOW_SHA,
                    "run_attempt": 1,
                    "run_started_at": started.isoformat(),
                    "updated_at": (started + timedelta(seconds=60)).isoformat(),
                },
            )
            write_json(
                job_json,
                {
                    "id": 88,
                    "run_id": 77,
                    "name": "Verify and stage complete prior website",
                    "status": "completed",
                    "conclusion": "success",
                    "head_sha": WORKFLOW_SHA,
                    "run_attempt": 1,
                    "started_at": started.isoformat(),
                    "completed_at": (started + timedelta(seconds=60)).isoformat(),
                },
            )
            validated = validate_dry_run_record(
                record_path=record_path,
                run_json=run_json,
                job_json=job_json,
                stage_identity=identity_path,
                repository=REPOSITORY,
                default_branch="main",
                expected_run_id=77,
                expected_run_attempt=1,
                target_kind=COMING_SOON,
                target_version=None,
                target_release_record_sha256=None,
                expected_live_version=NEXT_VERSION,
                expected_live_manifest_sha256=manifest_sha256,
                required_workflow_sha=WORKFLOW_SHA,
                required_observed_live_state="target_baseline",
            )
            self.assertEqual(validated["target"]["kind"], COMING_SOON)

    def test_rollback_staging_rejects_website_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            candidate = workspace / "candidate"
            _, record = signed_candidate(candidate)
            outside = workspace / "outside.txt"
            outside.write_text("must not be copied\n", encoding="utf-8")
            (candidate / "website" / "escape").symlink_to(outside)
            with self.assertRaisesRegex(ValueError, "symlink"):
                stage(
                    candidate=candidate,
                    release_record=record,
                    release_record_sha256=sha256(record),
                    version=VERSION,
                    output=workspace / "staged",
                    identity_output=workspace / "stage.json",
                )

    def test_workflows_fail_closed_for_history_and_rollback(self) -> None:
        candidate = (ROOT / ".github/workflows/flasher-candidate.yml").read_text(
            encoding="utf-8"
        )
        rollback = (ROOT / ".github/workflows/flasher-rollback.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("default: retain", candidate)
        self.assertIn("guard-bootstrap", candidate)
        self.assertIn("finalized flasher release record", candidate)
        self.assertIn(
            'record="$assets/flasher-release-record-v${HISTORY_VERSION}.json"',
            candidate,
        )
        self.assertLess(
            candidate.index("mkdir -p target"),
            candidate.index("> target/bootstrap-releases.json"),
        )
        self.assertIn("probe-stable", candidate)
        self.assertIn("cmp target/bootstrap-live/stable.json", candidate)
        self.assertIn("flasher-release-history-${{ github.run_id }}", candidate)
        self.assertIn("EXPECTED_HISTORY_SHA256", candidate)
        self.assertIn("needs: history", candidate)
        self.assertIn("PRNS_RELEASE_HISTORY", candidate)
        self.assertIn("targetCommitish", candidate)
        self.assertIn("./tools/prns release historical verify --", candidate)
        self.assertIn(
            'record="$assets/flasher-release-record-v${TARGET_VERSION}.json"',
            rollback,
        )
        self.assertIn("group: prns-public-pages", rollback)
        self.assertIn("environment: release-rollback", rollback)
        self.assertIn("timeout-minutes: 15", rollback)
        self.assertIn("validate-record", rollback)
        self.assertIn("--expected-run-attempt", rollback)
        self.assertIn('--required-workflow-sha "$GITHUB_SHA"', rollback)
        self.assertIn("./tools/prns release rollback -- live-state", rollback)
        self.assertIn('--mode "$ROLLBACK_MODE"', rollback)
        self.assertIn("--mode deploy", rollback)
        self.assertIn("targetCommitish", rollback)
        self.assertIn("./tools/prns release historical verify --", rollback)
        self.assertIn("run-id: ${{ inputs.dry_run_id }}", rollback)
        self.assertIn("dry_run_attempt:", rollback)
        self.assertIn(
            "flasher-rollback-dry-run-${{ inputs.dry_run_id }}-attempt-${{ inputs.dry_run_attempt }}",
            rollback,
        )
        self.assertIn(
            "actions/runs/${DRY_RUN_ID}/attempts/${DRY_RUN_ATTEMPT}", rollback
        )
        self.assertNotIn("overwrite: true", rollback)
        self.assertIn("actions/upload-pages-artifact@", rollback)
        self.assertIn("actions/deploy-pages@", rollback)
        self.assertIn("verify-live-website", rollback)
        self.assertIn("target/rollback-stage/rollback-stage.json", rollback)
        self.assertIn("cmp target/assets-before-latest.json", rollback)
        self.assertIn("target_kind:", rollback)
        self.assertIn("stage-coming-soon", rollback)
        self.assertIn("cas-coming-soon", rollback)
        self.assertIn("--prerelease=true --latest=false", rollback)
        self.assertIn("withdrawn-manifest-sha256", rollback)
        self.assertNotIn("PRNS_MINISIGN_SECRET_KEY_B64", rollback)
        self.assertNotIn("secrets.", rollback)

        promotion = (ROOT / ".github/workflows/flasher-promote.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("rollback_baseline_version", promotion)
        self.assertIn("rollback_baseline_kind", promotion)
        self.assertIn("rollback_baseline_release_record_sha256", promotion)
        self.assertIn("rollback_dry_run_id", promotion)
        self.assertIn("rollback_dry_run_attempt", promotion)
        self.assertIn("Block promotion without a matching <=15-minute rollback dry-run", promotion)
        self.assertIn("validate-record", promotion)
        self.assertIn('--required-workflow-sha "$GITHUB_SHA"', promotion)
        self.assertIn("--required-observed-live-state target_baseline", promotion)
        self.assertIn(".head.release_record_sha256", promotion)
        self.assertIn("targetCommitish", promotion)
        self.assertIn("./tools/prns release historical verify --", promotion)
        self.assertIn(
            'record="target/release/flasher-release-record-v${RELEASE_VERSION}.json"',
            promotion,
        )
        self.assertIn("--public-review-evidence", promotion)
        self.assertIn("--public-review-run", promotion)
        self.assertIn("Recheck live promotion CAS immediately before Pages deployment", promotion)
        self.assertIn("stage-coming-soon", promotion)
        self.assertIn("--baseline-kind", promotion)

        historical = (ROOT / "tools/release/verify-historical-flasher-release.py").read_text(
            encoding="utf-8"
        )
        self.assertLess(
            historical.index('"-Vm"'),
            historical.index("extract_source(policy_commit, snapshot)"),
        )
        self.assertLess(
            historical.index('"-Vm"'),
            historical.index('acceptance_record.get("source_commit")'),
        )
        self.assertIn('"merge-base", "--is-ancestor"', historical)
        self.assertIn("historical verifier uses a different release trust root", historical)
        self.assertIn("qualification-evidence-v{arguments.version}.tar.gz", historical)
        self.assertIn('"--prerelease-published-at"', historical)


if __name__ == "__main__":
    unittest.main()
