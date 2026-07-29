#!/usr/bin/env python3
"""Create or verify exact protected flasher public-review evidence."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from flasher_public_review import (
    build_evidence,
    discover_evidence,
    load_object,
    validate_evidence,
    write_evidence,
)


def add_common(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--release-json", type=Path, required=True)
    parser.add_argument("--run-json", type=Path, required=True)
    parser.add_argument("--job-json", type=Path, required=True)
    parser.add_argument("--signed-bundle", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--source-commit", required=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    create = subparsers.add_parser("create")
    add_common(create)
    create.add_argument("--approved-at", required=True)
    create.add_argument("--output", type=Path, required=True)
    verify = subparsers.add_parser("verify")
    add_common(verify)
    verify.add_argument("--evidence", type=Path, required=True)
    verify.add_argument("--allow-promoted", action="store_true")
    discover = subparsers.add_parser("discover")
    discover.add_argument("--evidence-directory", type=Path, required=True)
    discover.add_argument("--repository", required=True)
    discover.add_argument("--version", required=True)
    discover.add_argument("--source-commit", required=True)
    discover.add_argument("--workflow-run-id", type=int)
    discover.add_argument("--signed-candidate-sha256", required=True)
    discover.add_argument("--manifest-sha256", required=True)
    arguments = parser.parse_args()
    try:
        if arguments.command == "discover":
            paths = discover_evidence(
                arguments.evidence_directory,
                repository=arguments.repository,
                version=arguments.version,
                source_commit=arguments.source_commit,
                workflow_run_id=arguments.workflow_run_id,
                signed_candidate_sha256=arguments.signed_candidate_sha256,
                manifest_sha256=arguments.manifest_sha256,
            )
            for path in paths:
                print(path)
            return 0
        common = {
            "release": load_object(arguments.release_json, "public prerelease"),
            "run": load_object(arguments.run_json, "signing workflow run"),
            "job": load_object(arguments.job_json, "public-review workflow job"),
            "signed_bundle": arguments.signed_bundle,
            "manifest": arguments.manifest,
            "repository": arguments.repository,
            "version": arguments.version,
            "source_commit": arguments.source_commit,
        }
        if arguments.command == "create":
            evidence = build_evidence(approved_at=arguments.approved_at, **common)
            write_evidence(arguments.output, evidence)
            print(arguments.output)
        else:
            evidence = load_object(arguments.evidence, "public-review evidence")
            validate_evidence(
                evidence,
                allow_promoted=arguments.allow_promoted,
                **common,
            )
            print(
                f"verified protected public review for {evidence['version']} "
                f"in signing run {evidence['workflow_run_id']}"
            )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"public-review evidence validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
