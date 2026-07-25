#!/usr/bin/env python3
"""Require an immutable public candidate before protected promotion."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import sys


def parse_timestamp(value: object) -> datetime:
    if not isinstance(value, str):
        raise ValueError("prerelease publication time is missing")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError("prerelease publication time is malformed") from error
    if parsed.tzinfo is None:
        raise ValueError("prerelease publication time has no timezone")
    return parsed.astimezone(timezone.utc)


def validate(arguments: argparse.Namespace, now: datetime | None = None) -> None:
    release = json.loads(arguments.release_json.read_text(encoding="utf-8"))
    if not isinstance(release, dict):
        raise ValueError("GitHub release metadata must be a JSON object")
    if release.get("isDraft") is not False:
        raise ValueError("candidate release must remain public and non-draft")
    is_prerelease = release.get("isPrerelease")
    if is_prerelease is not True and not (
        getattr(arguments, "allow_promoted", False) and is_prerelease is False
    ):
        raise ValueError("candidate must be a prerelease unless exact promotion is resuming")
    if release.get("tagName") != f"v{arguments.version}":
        raise ValueError("prerelease tag differs from the qualified version")
    if release.get("targetCommitish") != arguments.source_commit:
        raise ValueError("prerelease tag target differs from the qualified source commit")
    published = parse_timestamp(release.get("publishedAt"))
    current = now or datetime.now(timezone.utc)
    if published > current:
        raise ValueError("prerelease publication time is in the future")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release-json", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--allow-promoted", action="store_true")
    arguments = parser.parse_args()
    try:
        validate(arguments)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"prerelease validation failed: {error}", file=sys.stderr)
        return 1
    print("verified immutable public release candidate")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
