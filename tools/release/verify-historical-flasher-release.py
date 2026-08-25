#!/usr/bin/env python3
"""Verify a signed release with the verifier frozen at its signed acceptance policy commit."""

from __future__ import annotations

import argparse
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile


ROOT = Path(__file__).resolve().parents[2]
COMMIT = re.compile(r"^[0-9a-f]{40}$")


def run(command: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> None:
    subprocess.run(command, cwd=cwd, env=env, check=True)


def existing(path: Path, label: str, *, directory: bool = False) -> Path:
    resolved = path.resolve(strict=True)
    if directory != resolved.is_dir():
        expected = "directory" if directory else "file"
        raise ValueError(f"{label} must be an existing {expected}")
    return resolved


def extract_source(source_commit: str, destination: Path) -> None:
    archive = subprocess.run(
        ["git", "-C", str(ROOT), "archive", "--format=tar", source_commit],
        check=True,
        stdout=subprocess.PIPE,
    ).stdout
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as source:
        source.extractall(destination, filter="data")


def release_asset_verifier(candidate: Path, historical_snapshot: Path) -> Path:
    """Select the asset policy without reviving the pre-hotfix suite requirement."""
    if (candidate / "metadata" / "hotfix.json").is_file():
        return existing(
            ROOT / "tools/release/verify-flasher-release-assets.py",
            "current hotfix-aware release-asset verifier",
        )
    return existing(
        historical_snapshot / "tools/release/verify-flasher-release-assets.py",
        "historical release-asset verifier",
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--candidate-run", type=Path, required=True)
    parser.add_argument("--signed-bundle", type=Path, required=True)
    parser.add_argument("--acceptance", type=Path, required=True)
    parser.add_argument("--acceptance-source-commit", required=True)
    parser.add_argument("--release-record", type=Path, required=True)
    parser.add_argument("--attestation-bundle", type=Path, required=True)
    parser.add_argument("--attestation-metadata", type=Path, required=True)
    parser.add_argument("--public-review-evidence", type=Path, required=True)
    parser.add_argument("--public-review-release", type=Path, required=True)
    parser.add_argument("--public-review-run", type=Path, required=True)
    parser.add_argument("--public-review-job", type=Path, required=True)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--version", required=True)
    arguments = parser.parse_args()
    try:
        signer = os.environ.get("PRNS_MINISIGN_BIN", "minisign")
        signer_path = shutil.which(signer)
        if signer_path is None:
            raise ValueError(f"configured Minisign executable is unavailable: {signer}")
        public_key = existing(
            Path(os.environ.get("PRNS_MINISIGN_PUBLIC_KEY", ROOT / "release/keys/minisign.pub")),
            "current release public key",
        )
        release_record = existing(arguments.release_record, "release record")
        release_signature = existing(
            Path(f"{release_record}.minisig"), "release-record signature"
        )
        run(
            [
                signer_path,
                "-Vm",
                str(release_record),
                "-x",
                str(release_signature),
                "-p",
                str(public_key),
            ]
        )
        record = json.loads(release_record.read_text(encoding="utf-8"))
        release = record.get("release") if isinstance(record, dict) else None
        if not isinstance(release, dict):
            raise ValueError("signed release record has no release identity")
        source_commit = release.get("source_commit")
        if not isinstance(source_commit, str) or COMMIT.fullmatch(source_commit) is None:
            raise ValueError("signed historical source commit is malformed")
        if release.get("version") != arguments.version:
            raise ValueError("signed release record differs from the requested historical version")
        prerelease_published_at = release.get("prerelease_published_at")
        if not isinstance(prerelease_published_at, str) or not prerelease_published_at:
            raise ValueError("signed release record has no prerelease publication time")
        acceptance_record = record.get("acceptance")
        policy_commit = (
            acceptance_record.get("source_commit")
            if isinstance(acceptance_record, dict)
            else None
        )
        if not isinstance(policy_commit, str) or COMMIT.fullmatch(policy_commit) is None:
            policy_commit = source_commit
        run(["git", "-C", str(ROOT), "cat-file", "-e", f"{source_commit}^{{commit}}"])
        run(["git", "-C", str(ROOT), "merge-base", "--is-ancestor", source_commit, "HEAD"])
        run(["git", "-C", str(ROOT), "cat-file", "-e", f"{policy_commit}^{{commit}}"])
        run(["git", "-C", str(ROOT), "merge-base", "--is-ancestor", policy_commit, "HEAD"])

        assets = existing(arguments.assets, "release assets", directory=True)
        paths = {
            "candidate": existing(arguments.candidate, "candidate", directory=True),
            "candidate_run": existing(arguments.candidate_run, "candidate run"),
            "signed_bundle": existing(arguments.signed_bundle, "signed bundle"),
            "acceptance": existing(arguments.acceptance, "acceptance"),
            "attestation_bundle": existing(arguments.attestation_bundle, "attestation bundle"),
            "attestation_metadata": existing(
                arguments.attestation_metadata, "attestation metadata"
            ),
            "public_review_evidence": existing(
                arguments.public_review_evidence, "public-review evidence"
            ),
            "public_review_release": existing(
                arguments.public_review_release, "public-review release state"
            ),
            "public_review_run": existing(
                arguments.public_review_run, "public-review workflow run"
            ),
            "public_review_job": existing(
                arguments.public_review_job, "public-review workflow job"
            ),
            "qualification_evidence": existing(
                assets / f"qualification-evidence-v{arguments.version}.tar.gz",
                "qualification evidence",
            ),
            "assets": assets,
        }
        with tempfile.TemporaryDirectory(prefix="prns-historical-verifier-") as temporary:
            snapshot = Path(temporary)
            extract_source(policy_commit, snapshot)
            historical_key = existing(
                snapshot / "release/keys/minisign.pub", "historical release public key"
            )
            if historical_key.read_bytes() != public_key.read_bytes():
                raise ValueError("historical verifier uses a different release trust root")
            verifier = existing(
                snapshot / "tools/release/verify-flasher-release.sh", "historical release verifier"
            )
            asset_verifier = release_asset_verifier(paths["candidate"], snapshot)
            public_review_verifier = existing(
                snapshot / "tools/release/flasher-public-review.py",
                "historical public-review verifier",
            )
            environment = dict(os.environ)
            environment["PRNS_MINISIGN_BIN"] = signer_path
            environment["PRNS_MINISIGN_PUBLIC_KEY"] = str(public_key)
            run(
                [
                    str(verifier),
                    "--candidate",
                    str(paths["candidate"]),
                    "--candidate-run",
                    str(paths["candidate_run"]),
                    "--signed-bundle",
                    str(paths["signed_bundle"]),
                    "--acceptance",
                    str(paths["acceptance"]),
                    "--acceptance-source-commit",
                    arguments.acceptance_source_commit,
                    "--qualification-evidence",
                    str(paths["qualification_evidence"]),
                    "--public-review-evidence",
                    str(paths["public_review_evidence"]),
                    "--prerelease-published-at",
                    prerelease_published_at,
                    "--release-record",
                    str(release_record),
                    "--attestation-bundle",
                    str(paths["attestation_bundle"]),
                    "--attestation-metadata",
                    str(paths["attestation_metadata"]),
                    "--repository",
                    arguments.repository,
                ],
                cwd=snapshot,
                env=environment,
            )
            run(
                [
                    sys.executable,
                    str(public_review_verifier),
                    "verify",
                    "--evidence",
                    str(paths["public_review_evidence"]),
                    "--release-json",
                    str(paths["public_review_release"]),
                    "--run-json",
                    str(paths["public_review_run"]),
                    "--job-json",
                    str(paths["public_review_job"]),
                    "--signed-bundle",
                    str(paths["signed_bundle"]),
                    "--manifest",
                    str(paths["candidate"] / "flash-manifest.json"),
                    "--repository",
                    arguments.repository,
                    "--version",
                    arguments.version,
                    "--source-commit",
                    source_commit,
                    "--allow-promoted",
                ],
                cwd=snapshot,
                env=environment,
            )
            run(
                [
                    sys.executable,
                    str(asset_verifier),
                    "--candidate",
                    str(paths["candidate"]),
                    "--assets",
                    str(paths["assets"]),
                    "--version",
                    arguments.version,
                ],
                cwd=snapshot,
                env=environment,
            )
    except (
        OSError,
        ValueError,
        json.JSONDecodeError,
        subprocess.CalledProcessError,
        tarfile.TarError,
    ) as error:
        print(f"historical flasher release verification failed: {error}", file=sys.stderr)
        return 1
    print(
        f"verified historical flasher release {arguments.version} at its signed acceptance policy"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
