#!/usr/bin/env python3
"""Bind a downloaded candidate to one successful default-branch workflow run."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys


WORKFLOW_PATH = ".github/workflows/flasher-candidate.yml"


def validate(arguments: argparse.Namespace) -> dict:
    run = json.loads(arguments.run_json.read_text(encoding="utf-8"))
    manifest = json.loads(arguments.manifest.read_text(encoding="utf-8"))
    if not isinstance(run, dict) or not isinstance(manifest, dict):
        raise ValueError("workflow run and manifest must be JSON objects")
    try:
        expected_run_id = int(arguments.expected_run_id)
    except ValueError as error:
        raise ValueError("candidate run ID must be an integer") from error
    release = manifest.get("release")
    if not isinstance(release, dict) or not isinstance(release.get("commit"), str):
        raise ValueError("candidate manifest has no source commit")
    repository = run.get("repository")
    head_repository = run.get("head_repository")
    workflow_path = run.get("path")
    allowed_paths = {
        WORKFLOW_PATH,
        f"{WORKFLOW_PATH}@refs/heads/{arguments.default_branch}",
    }
    checks = {
        "run ID": run.get("id") == expected_run_id,
        "repository": isinstance(repository, dict)
        and repository.get("full_name") == arguments.repository,
        "head repository": isinstance(head_repository, dict)
        and head_repository.get("full_name") == arguments.repository,
        "workflow path": workflow_path in allowed_paths,
        "event": run.get("event") == "workflow_dispatch",
        "status": run.get("status") == "completed",
        "conclusion": run.get("conclusion") == "success",
        "default branch": run.get("head_branch") == arguments.default_branch,
        "source commit": run.get("head_sha") == release["commit"],
    }
    failed = [name for name, passed in checks.items() if not passed]
    if failed:
        raise ValueError(f"candidate workflow run failed custody checks: {failed}")
    source_commit = release["commit"]
    if len(source_commit) != 40 or any(character not in "0123456789abcdef" for character in source_commit):
        raise ValueError("candidate source commit must be a lowercase full Git commit")
    run_attempt = run.get("run_attempt")
    if not isinstance(run_attempt, int) or isinstance(run_attempt, bool) or run_attempt <= 0:
        raise ValueError("candidate workflow run attempt must be a positive integer")
    return {
        "schema": 1,
        "repository": arguments.repository,
        "workflow_path": WORKFLOW_PATH,
        "workflow_run_id": expected_run_id,
        "workflow_run_attempt": run_attempt,
        "source_commit": source_commit,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-json", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--expected-run-id", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--default-branch", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        identity = validate(arguments)
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        arguments.output.write_text(
            json.dumps(identity, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
            newline="\n",
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"candidate workflow custody validation failed: {error}", file=sys.stderr)
        return 1
    print(f"verified candidate workflow run {identity['workflow_run_id']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
