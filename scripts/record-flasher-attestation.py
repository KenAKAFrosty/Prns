#!/usr/bin/env python3
"""Capture the exact GitHub attestation identity and subjects for release custody."""

from __future__ import annotations

import argparse
import json
from pathlib import Path, PurePosixPath
import sys

from flasher_release_evidence import attestation_subjects, sha256


def canonical_name(value: str) -> str:
    path = PurePosixPath(value)
    if (
        "\\" in value
        or path.is_absolute()
        or not path.parts
        or any(part in {"", ".", ".."} for part in path.parts)
        or path.as_posix() != value
    ):
        raise ValueError(f"attestation subject name is not canonical: {value!r}")
    return value

def build(arguments: argparse.Namespace) -> dict:
    bundle = json.loads(arguments.bundle.read_text(encoding="utf-8"))
    if not isinstance(bundle, dict):
        raise ValueError("attestation bundle must be a JSON object")
    subjects = attestation_subjects(bundle)
    actual_subjects = {(subject["name"], subject["sha256"]) for subject in subjects}
    required_subjects = set()
    required_paths = set()
    for raw_name, raw_path in arguments.required_subject:
        name = canonical_name(raw_name)
        path = Path(raw_path)
        if not path.is_file():
            raise ValueError(f"required attestation subject is unavailable: {path}")
        resolved = path.resolve()
        if resolved in required_paths:
            raise ValueError(f"required attestation subject was repeated: {path}")
        required_paths.add(resolved)
        identity = (name, sha256(path))
        if any(existing[0] == name for existing in required_subjects):
            raise ValueError(f"required attestation subject name was repeated: {name}")
        required_subjects.add(identity)
    if actual_subjects != required_subjects:
        missing = sorted(required_subjects - actual_subjects)
        unexpected = sorted(actual_subjects - required_subjects)
        raise ValueError(
            f"attestation subjects differ from exact canonical inputs; "
            f"missing={missing}, unexpected={unexpected}"
        )

    expected_workflow_prefix = (
        f"{arguments.repository}/.github/workflows/flasher-sign.yml@refs/heads/"
    )
    if not arguments.workflow_ref.startswith(expected_workflow_prefix):
        raise ValueError("attestation workflow identity is not the protected flasher signer")
    if not arguments.attestation_id.strip() or len(arguments.attestation_id) > 128:
        raise ValueError("attestation ID is malformed")
    expected_url_prefix = f"https://github.com/{arguments.repository}/attestations/"
    if not arguments.attestation_url.startswith(expected_url_prefix):
        raise ValueError("attestation URL is outside the release repository")
    try:
        run_id = int(arguments.workflow_run_id)
    except ValueError as error:
        raise ValueError("attestation workflow run ID must be an integer") from error
    if run_id <= 0:
        raise ValueError("attestation workflow run ID must be positive")
    workflow_sha = arguments.workflow_sha
    if len(workflow_sha) != 40 or any(
        character not in "0123456789abcdef" for character in workflow_sha
    ):
        raise ValueError("attestation workflow SHA must be a lowercase full Git commit")
    return {
        "schema": 1,
        "repository": arguments.repository,
        "workflow_ref": arguments.workflow_ref,
        "workflow_sha": workflow_sha,
        "workflow_run_id": run_id,
        "attestation_id": arguments.attestation_id,
        "attestation_url": arguments.attestation_url,
        "bundle": {"name": arguments.bundle.name, "sha256": sha256(arguments.bundle)},
        "subjects": subjects,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument(
        "--required-subject",
        nargs=2,
        action="append",
        required=True,
        metavar=("NAME", "PATH"),
    )
    parser.add_argument("--repository", required=True)
    parser.add_argument("--workflow-ref", required=True)
    parser.add_argument("--workflow-sha", required=True)
    parser.add_argument("--workflow-run-id", required=True)
    parser.add_argument("--attestation-id", required=True)
    parser.add_argument("--attestation-url", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        metadata = build(arguments)
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        arguments.output.write_text(
            json.dumps(metadata, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
            newline="\n",
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"attestation recording failed: {error}", file=sys.stderr)
        return 1
    print(arguments.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
