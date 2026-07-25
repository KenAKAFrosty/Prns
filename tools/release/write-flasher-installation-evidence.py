#!/usr/bin/env python3

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import sys

from flasher_tester_roster import validate_roster


TARGET_EXTENSIONS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-apple-darwin": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
}
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
COMMIT_PATTERN = re.compile(r"^[0-9a-f]{40}$")
REPOSITORY_PATTERN = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")


def timestamp(value: str, label: str) -> datetime:
    normalized = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError as error:
        raise ValueError(f"{label} must be a full ISO timestamp") from error
    if parsed.tzinfo is None:
        raise ValueError(f"{label} must include a timezone")
    return parsed.astimezone(timezone.utc)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def positive_integer(value: str, label: str) -> int:
    try:
        parsed = int(value)
    except ValueError as error:
        raise ValueError(f"{label} must be a positive integer") from error
    if parsed < 1:
        raise ValueError(f"{label} must be a positive integer")
    return parsed


def create(arguments: argparse.Namespace, now: datetime | None = None) -> dict:
    if arguments.target not in TARGET_EXTENSIONS:
        raise ValueError("target is not a published CLI target")
    if not COMMIT_PATTERN.fullmatch(arguments.source_commit):
        raise ValueError("source commit must be a lowercase full Git SHA")
    if arguments.workflow_sha != arguments.source_commit:
        raise ValueError("workflow SHA must equal the candidate source commit")
    if not SHA256_PATTERN.fullmatch(arguments.signed_candidate_sha256):
        raise ValueError("signed candidate SHA-256 must be lowercase")
    if not SHA256_PATTERN.fullmatch(arguments.expected_archive_sha256):
        raise ValueError("expected archive SHA-256 must be lowercase")
    expected_archive_name = (
        f"hopspot-flash-{arguments.version}-{arguments.target}"
        f"{TARGET_EXTENSIONS[arguments.target]}"
    )
    if arguments.archive.name != expected_archive_name:
        raise ValueError("archive name differs from the exact target and version")
    actual_archive_sha256 = sha256(arguments.archive)
    if actual_archive_sha256 != arguments.expected_archive_sha256:
        raise ValueError("archive bytes differ from the signed expected SHA-256")
    expected_version_output = f"hopspot-flash {arguments.version}"
    if arguments.version_output != expected_version_output:
        raise ValueError("installed CLI reported a different version")
    roster_document = json.loads(arguments.roster.read_text(encoding="utf-8"))
    roster, roster_errors = validate_roster(roster_document, arguments.version)
    if roster_errors:
        raise ValueError(f"tester roster is invalid: {roster_errors}")
    assignment = roster.installations.get(arguments.target)
    if assignment is None:
        raise ValueError("target is absent from the exact tester roster")
    published_at = timestamp(arguments.published_at, "published-at")
    completed_at = timestamp(arguments.completed_at, "completed-at")
    current = (now or datetime.now(timezone.utc)).astimezone(timezone.utc)
    if completed_at < published_at:
        raise ValueError("installation observation predates public release publication")
    if completed_at > current:
        raise ValueError("installation observation cannot be in the future")
    run_id = positive_integer(arguments.workflow_run_id, "workflow run ID")
    run_attempt = positive_integer(
        arguments.workflow_run_attempt,
        "workflow run attempt",
    )
    if not REPOSITORY_PATTERN.fullmatch(arguments.repository):
        raise ValueError("repository must be an owner/name identity")
    if not arguments.workflow_job or any(
        ord(character) < 0x20 for character in arguments.workflow_job
    ):
        raise ValueError("workflow job must be a nonempty identity")
    if not arguments.os_version.strip():
        raise ValueError("OS version must be nonempty")
    evidence = {
        "schema": 1,
        "kind": "native-installation-smoke",
        "candidate": {
            "version": arguments.version,
            "source_commit": arguments.source_commit,
            "signed_candidate_sha256": arguments.signed_candidate_sha256,
            "published_at": arguments.published_at,
        },
        "assignment": {
            "target": assignment.target,
            "os": assignment.os_name,
            "architecture": assignment.architecture,
            "tester": assignment.tester,
        },
        "archive": {
            "name": expected_archive_name,
            "sha256": actual_archive_sha256,
            "url": (
                f"https://github.com/{arguments.repository}/releases/download/"
                f"v{arguments.version}/{expected_archive_name}"
            ),
        },
        "observations": {
            "install": "pass",
            "version": "pass",
            "version_output": arguments.version_output,
            "os_version": arguments.os_version,
        },
        "workflow": {
            "repository": arguments.repository,
            "run_id": run_id,
            "run_attempt": run_attempt,
            "job": arguments.workflow_job,
            "sha": arguments.workflow_sha,
        },
        "completed_at": arguments.completed_at,
    }
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    with arguments.output.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(evidence, stream, indent=2, sort_keys=True)
        stream.write("\n")
    return evidence


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--roster", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--signed-candidate-sha256", required=True)
    parser.add_argument("--published-at", required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--expected-archive-sha256", required=True)
    parser.add_argument("--version-output", required=True)
    parser.add_argument("--os-version", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--workflow-run-id", required=True)
    parser.add_argument("--workflow-run-attempt", required=True)
    parser.add_argument("--workflow-job", required=True)
    parser.add_argument("--workflow-sha", required=True)
    parser.add_argument("--completed-at", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        create(arguments)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"installation evidence creation failed: {error}", file=sys.stderr)
        return 1
    print(arguments.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
