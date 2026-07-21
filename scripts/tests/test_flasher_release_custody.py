from __future__ import annotations

import argparse
import base64
from datetime import datetime, timedelta, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
VERSION = "0.2.6"
SOURCE_COMMIT = "a" * 40
ACCEPTANCE_COMMIT = "b" * 40
KEY_ID = "0123456789ABCDEF"
REPOSITORY = "example/Prns"
CLI_TARGETS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-apple-darwin": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_script(script: str, *arguments: object, environment: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    command = [str(SCRIPTS / script), *(str(argument) for argument in arguments)]
    return subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def fake_signer(path: Path) -> None:
    path.write_text(
        """#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "-S" ]]; then
  document=""
  signature=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -m) document="$2"; shift 2 ;;
      -x) signature="$2"; shift 2 ;;
      *) shift ;;
    esac
  done
  test -f "$document"
  printf 'fixture-signature:%s\n' "$(sha256sum "$document" | awk '{print $1}')" > "$signature"
  exit 0
fi
exit 0
""",
        encoding="utf-8",
    )
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


class CandidateFixture:
    def __init__(self, root: Path) -> None:
        self.root = root
        root.mkdir(parents=True)
        self.key = root.parent / "minisign.pub"
        self.repository_version = root.parent / "VERSION"
        self.key.write_text(
            f"untrusted comment: minisign public key {KEY_ID}\nRWQfixturepublickey\n",
            encoding="utf-8",
        )
        self.repository_version.write_text(f"{VERSION}\n", encoding="utf-8")
        (root / "minisign.pub").write_bytes(self.key.read_bytes())
        (root / "VERSION").write_text(f"{VERSION}\n", encoding="utf-8")
        (root / "website").mkdir(parents=True)
        (root / "website" / "index.html").write_text("fixture site\n", encoding="utf-8")
        targets = []
        for index, board in enumerate(
            ("heltec-v4", "t-beam-supreme", "xiao-esp32-c6", "t-echo"), start=1
        ):
            relative = f"firmware/{board}/application.bin"
            artifact = root / relative
            artifact.parent.mkdir(parents=True, exist_ok=True)
            artifact.write_bytes(f"firmware-{index}-{board}".encode())
            hosted = root / "website" / "releases" / VERSION / relative
            hosted.parent.mkdir(parents=True, exist_ok=True)
            hosted.write_bytes(artifact.read_bytes())
            targets.append(
                {
                    "board_slug": board,
                    "parts": [
                        {
                            "path": relative,
                            "size": artifact.stat().st_size,
                            "sha256": sha256(artifact),
                        }
                    ],
                }
            )
        self.manifest = {
            "schema": 2,
            "release": {
                "version": VERSION,
                "channel": "stable",
                "commit": SOURCE_COMMIT,
            },
            "signing": {"key_id": KEY_ID},
            "targets": targets,
        }
        self.manifest_path = root / "flash-manifest.json"
        write_json(self.manifest_path, self.manifest)
        hosted_manifest = root / "website" / "releases" / VERSION / "flash-manifest.json"
        hosted_manifest.write_bytes(self.manifest_path.read_bytes())
        self.channel = {
            "schema": 1,
            "channel": "stable",
            "version": VERSION,
            "manifest_url": f"https://reticulum.rs/releases/{VERSION}/flash-manifest.json",
            "manifest_sha256": sha256(self.manifest_path),
        }
        self.channel_path = root / "channels" / "stable.json"
        write_json(self.channel_path, self.channel)
        hosted_channel = root / "website" / "releases" / "channels" / "stable.json"
        hosted_channel.parent.mkdir(parents=True, exist_ok=True)
        hosted_channel.write_bytes(self.channel_path.read_bytes())
        write_json(
            root / "metadata" / "build.json",
            {
                "schema": 1,
                "source_commit": SOURCE_COMMIT,
                "built_at_utc": "2026-07-21T12:00:00+00:00",
                "host": {"system": "Linux", "machine": "x86_64"},
                "tools": {"rustc": "fixture"},
            },
        )
        audit = root / "audit" / "release-audit-evidence.md"
        audit.parent.mkdir(parents=True, exist_ok=True)
        audit.write_text("fixture release audit\n", encoding="utf-8")
        cli = root / "cli"
        cli.mkdir()
        self.cli_archives = []
        for index, (target, extension) in enumerate(CLI_TARGETS.items(), start=1):
            archive = cli / f"hopspot-flash-{VERSION}-{target}{extension}"
            archive.write_bytes(f"cli-{index}-{target}".encode())
            self.cli_archives.append(archive)
        (cli / "install.sh").write_text("#!/bin/sh\n", encoding="utf-8")
        (cli / "install.ps1").write_text("# fixture\n", encoding="utf-8")
        (cli / "README.md").write_text("fixture\n", encoding="utf-8")
        self.write_sums()

    def write_sums(self) -> None:
        files = sorted(
            path
            for path in self.root.rglob("*")
            if path.is_file() and path.name != "SHA256SUMS.txt" and not path.name.endswith(".minisig")
        )
        (self.root / "SHA256SUMS.txt").write_text(
            "".join(f"{sha256(path)}  {path.relative_to(self.root).as_posix()}\n" for path in files),
            encoding="utf-8",
        )


class FlasherReleaseCustodyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.workspace = Path(self.temporary.name)
        self.fixture = CandidateFixture(self.workspace / "candidate")
        self.signer = self.workspace / "fake-minisign"
        self.secret = self.workspace / "fixture.key"
        fake_signer(self.signer)
        self.secret.write_text("fixture secret\n", encoding="utf-8")
        self.environment = dict(os.environ)
        self.environment.update(
            {
                "PRNS_MINISIGN_BIN": str(self.signer),
                "PRNS_MINISIGN_PUBLIC_KEY": str(self.fixture.key),
            }
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def validate_unsigned(self) -> subprocess.CompletedProcess[str]:
        return run_script(
            "validate-unsigned-flasher-candidate.py",
            self.fixture.root,
            "--expected-commit",
            SOURCE_COMMIT,
            "--repository-version",
            self.fixture.repository_version,
            "--pinned-key",
            self.fixture.key,
        )

    def sign_candidate(self) -> subprocess.CompletedProcess[str]:
        return run_script(
            "sign-flasher-candidate.sh",
            self.fixture.root,
            self.secret,
            environment=self.environment,
        )

    def test_unsigned_candidate_is_fully_bound_before_signing(self) -> None:
        result = self.validate_unsigned()
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = self.fixture.cli_archives[0]
        archive.write_bytes(b"tampered")
        result = self.validate_unsigned()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA-256 mismatch", result.stderr)

    def test_fake_signer_injection_signs_documents_and_hosted_copies(self) -> None:
        result = self.sign_candidate()
        self.assertEqual(result.returncode, 0, result.stderr)
        for document in (
            self.fixture.manifest_path,
            self.fixture.channel_path,
            self.fixture.root / "SHA256SUMS.txt",
        ):
            self.assertTrue(Path(f"{document}.minisig").is_file())
        self.assertEqual(
            (self.fixture.root / "website" / "releases" / VERSION / "flash-manifest.json.minisig").read_bytes(),
            Path(f"{self.fixture.manifest_path}.minisig").read_bytes(),
        )
        rerun = self.sign_candidate()
        self.assertNotEqual(rerun.returncode, 0)
        self.assertIn("existing signature", rerun.stderr)

    def test_signed_candidate_packaging_is_deterministic(self) -> None:
        self.assertEqual(self.sign_candidate().returncode, 0)
        first = self.workspace / "first.tar.gz"
        second = self.workspace / "second.tar.gz"
        self.assertEqual(
            run_script("package-flasher-candidate.py", self.fixture.root, first).returncode, 0
        )
        for path in self.fixture.root.rglob("*"):
            os.utime(path, (1_900_000_000, 1_900_000_000), follow_symlinks=False)
        self.assertEqual(
            run_script("package-flasher-candidate.py", self.fixture.root, second).returncode, 0
        )
        self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_candidate_run_must_be_successful_default_branch_provenance(self) -> None:
        run_document = {
            "id": 42,
            "repository": {"full_name": REPOSITORY},
            "head_repository": {"full_name": REPOSITORY},
            "path": ".github/workflows/flasher-candidate.yml",
            "event": "workflow_dispatch",
            "status": "completed",
            "conclusion": "success",
            "head_branch": "main",
            "head_sha": SOURCE_COMMIT,
            "run_attempt": 1,
        }
        run_json = self.workspace / "run.json"
        output = self.workspace / "run-identity.json"
        write_json(run_json, run_document)
        result = run_script(
            "validate-flasher-candidate-run.py",
            "--run-json",
            run_json,
            "--manifest",
            self.fixture.manifest_path,
            "--expected-run-id",
            "42",
            "--repository",
            REPOSITORY,
            "--default-branch",
            "main",
            "--output",
            output,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        run_document["head_branch"] = "feature"
        write_json(run_json, run_document)
        rejected = run_script(
            "validate-flasher-candidate-run.py",
            "--run-json",
            run_json,
            "--manifest",
            self.fixture.manifest_path,
            "--expected-run-id",
            "42",
            "--repository",
            REPOSITORY,
            "--default-branch",
            "main",
            "--output",
            output,
        )
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("default branch", rejected.stderr)

    def make_release_evidence(self) -> tuple[Path, Path, Path, Path, Path]:
        self.assertEqual(self.sign_candidate().returncode, 0)
        signed_bundle = self.workspace / f"prns-flasher-candidate-v{VERSION}-signed.tar.gz"
        result = run_script(
            "package-flasher-candidate.py", self.fixture.root, signed_bundle
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        subject_paths = [signed_bundle, *self.fixture.cli_archives]
        statement = {
            "_type": "https://in-toto.io/Statement/v1",
            "subject": [
                {"name": path.name, "digest": {"sha256": sha256(path)}}
                for path in subject_paths
            ],
            "predicateType": "https://slsa.dev/provenance/v1",
            "predicate": {},
        }
        bundle = self.workspace / f"prns-flasher-attestation-v{VERSION}.json"
        write_json(
            bundle,
            {
                "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
                "dsseEnvelope": {
                    "payloadType": "application/vnd.in-toto+json",
                    "payload": base64.b64encode(json.dumps(statement).encode()).decode(),
                    "signatures": [{"sig": "fixture"}],
                },
            },
        )
        metadata = self.workspace / f"prns-flasher-attestation-v{VERSION}.metadata.json"
        arguments: list[object] = ["--bundle", bundle]
        for subject in subject_paths:
            arguments.extend(("--required-subject", subject))
        arguments.extend(
            (
                "--repository",
                REPOSITORY,
                "--workflow-ref",
                f"{REPOSITORY}/.github/workflows/flasher-sign.yml@refs/heads/main",
                "--workflow-run-id",
                "77",
                "--attestation-id",
                "12345",
                "--attestation-url",
                f"https://github.com/{REPOSITORY}/attestations/12345",
                "--output",
                metadata,
            )
        )
        result = run_script("record-flasher-attestation.py", *arguments)
        self.assertEqual(result.returncode, 0, result.stderr)
        acceptance = self.workspace / f"acceptance-v{VERSION}.json"
        write_json(
            acceptance,
            {
                "schema": 2,
                "candidate": {
                    "version": VERSION,
                    "channel": "stable",
                    "source_commit": SOURCE_COMMIT,
                    "signing_key_id": KEY_ID,
                    "manifest_sha256": sha256(self.fixture.manifest_path),
                    "manifest_signature_sha256": sha256(
                        Path(f"{self.fixture.manifest_path}.minisig")
                    ),
                },
            },
        )
        result = run_script(
            "sign-flasher-document.sh",
            acceptance,
            self.secret,
            environment=self.environment,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        candidate_run = self.workspace / f"prns-flasher-candidate-run-v{VERSION}.json"
        write_json(
            candidate_run,
            {
                "schema": 1,
                "repository": REPOSITORY,
                "workflow_path": ".github/workflows/flasher-candidate.yml",
                "workflow_run_id": 42,
                "workflow_run_attempt": 3,
                "source_commit": SOURCE_COMMIT,
            },
        )
        return signed_bundle, bundle, metadata, acceptance, candidate_run

    def test_release_record_binds_candidate_acceptance_audit_and_attestation(self) -> None:
        signed_bundle, bundle, metadata, acceptance, candidate_run = self.make_release_evidence()
        record = self.workspace / f"release-record-v{VERSION}.json"
        common: list[object] = [
            "--candidate",
            self.fixture.root,
            "--candidate-run",
            candidate_run,
            "--signed-bundle",
            signed_bundle,
            "--acceptance",
            acceptance,
            "--acceptance-source-commit",
            ACCEPTANCE_COMMIT,
            "--attestation-bundle",
            bundle,
            "--attestation-metadata",
            metadata,
            "--repository",
            REPOSITORY,
        ]
        created = run_script("flasher-release-record.py", "create", *common, "--output", record)
        self.assertEqual(created.returncode, 0, created.stderr)
        verified = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertEqual(verified.returncode, 0, verified.stderr)
        acceptance.write_text(acceptance.read_text() + " ", encoding="utf-8")
        rejected = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("release record does not match", rejected.stderr)

    def test_release_record_rejects_candidate_run_tamper_and_identity_mismatch(self) -> None:
        signed_bundle, bundle, metadata, acceptance, candidate_run = self.make_release_evidence()
        record = self.workspace / f"release-record-v{VERSION}.json"
        common: list[object] = [
            "--candidate",
            self.fixture.root,
            "--candidate-run",
            candidate_run,
            "--signed-bundle",
            signed_bundle,
            "--acceptance",
            acceptance,
            "--acceptance-source-commit",
            ACCEPTANCE_COMMIT,
            "--attestation-bundle",
            bundle,
            "--attestation-metadata",
            metadata,
            "--repository",
            REPOSITORY,
        ]
        created = run_script("flasher-release-record.py", "create", *common, "--output", record)
        self.assertEqual(created.returncode, 0, created.stderr)

        run_evidence = json.loads(candidate_run.read_text(encoding="utf-8"))
        run_evidence["workflow_run_attempt"] = 4
        write_json(candidate_run, run_evidence)
        tampered = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertNotEqual(tampered.returncode, 0)
        self.assertIn("release record does not match", tampered.stderr)

        run_evidence["source_commit"] = "c" * 40
        write_json(candidate_run, run_evidence)
        mismatched = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertNotEqual(mismatched.returncode, 0)
        self.assertIn("source commit differs", mismatched.stderr)

    def test_public_review_gate_uses_release_time_and_exact_commit(self) -> None:
        script = SCRIPTS / "validate-flasher-prerelease.py"
        spec = importlib.util.spec_from_file_location("validate_flasher_prerelease", script)
        if spec is None or spec.loader is None:
            self.fail(f"could not import {script}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        now = datetime(2026, 7, 21, 12, 0, tzinfo=timezone.utc)
        release_json = self.workspace / "release.json"
        write_json(
            release_json,
            {
                "isDraft": False,
                "isPrerelease": True,
                "tagName": f"v{VERSION}",
                "targetCommitish": SOURCE_COMMIT,
                "publishedAt": (now - timedelta(hours=24)).isoformat(),
            },
        )
        arguments = argparse.Namespace(
            release_json=release_json,
            version=VERSION,
            source_commit=SOURCE_COMMIT,
            minimum_hours=24,
        )
        module.validate(arguments, now=now)
        release = json.loads(release_json.read_text())
        release["publishedAt"] = (now - timedelta(hours=23, minutes=59)).isoformat()
        write_json(release_json, release)
        with self.assertRaisesRegex(ValueError, "shorter than 24 hours"):
            module.validate(arguments, now=now)

    def test_workflows_preserve_exact_candidate_custody_boundaries(self) -> None:
        candidate = (ROOT / ".github/workflows/flasher-candidate.yml").read_text()
        signing = (ROOT / ".github/workflows/flasher-sign.yml").read_text()
        evidence = (ROOT / ".github/workflows/flasher-finalize-evidence.yml").read_text()
        promotion = (ROOT / ".github/workflows/flasher-promote.yml").read_text()
        self.assertNotIn("gh release create", candidate)
        self.assertIn("candidate_run_id:", signing)
        self.assertIn("unsigned_bundle_sha256:", signing)
        self.assertIn("environment: release-signing", signing)
        self.assertIn("PRNS_MINISIGN_SECRET_KEY_B64", signing)
        self.assertIn(
            "actions/attest@59d89421af93a897026c735860bf21b6eb4f7b26", signing
        )
        self.assertIn("artifact-metadata: write", signing)
        self.assertIn("prns-flasher-candidate-run-v", signing)
        for forbidden in ("build-flasher-candidate.sh", "cargo build", "npm run", "dx build"):
            self.assertNotIn(forbidden, signing)
        self.assertIn("release/acceptance/records/${RELEASE_VERSION}.json", evidence)
        self.assertIn("flasher-release-record.py create", evidence)
        self.assertIn("--candidate-run", evidence)
        self.assertIn("sign-flasher-document.sh \"$record\"", evidence)
        self.assertIn("release_record_sha256:", promotion)
        self.assertIn("verify-flasher-release.sh", promotion)
        self.assertIn("--candidate-run", promotion)
        self.assertIn("--minimum-hours 24", promotion)


if __name__ == "__main__":
    unittest.main()
