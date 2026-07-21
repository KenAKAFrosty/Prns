#!/usr/bin/env python3
"""Require an immutable prerelease and a complete public review interval before promotion."""

from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
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
    if release.get("isDraft") is not False or release.get("isPrerelease") is not True:
        raise ValueError("candidate must remain a public, non-draft prerelease")
    if release.get("tagName") != f"v{arguments.version}":
        raise ValueError("prerelease tag differs from the qualified version")
    if release.get("targetCommitish") != arguments.source_commit:
        raise ValueError("prerelease tag target differs from the qualified source commit")
    published = parse_timestamp(release.get("publishedAt"))
    current = now or datetime.now(timezone.utc)
    if published > current:
        raise ValueError("prerelease publication time is in the future")
    if current - published < timedelta(hours=arguments.minimum_hours):
        raise ValueError(
            f"public review interval is shorter than {arguments.minimum_hours} hours"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release-json", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--minimum-hours", type=int, default=24)
    arguments = parser.parse_args()
    try:
        if arguments.minimum_hours < 1:
            raise ValueError("minimum public review hours must be positive")
        validate(arguments)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"prerelease validation failed: {error}", file=sys.stderr)
        return 1
    print(f"verified {arguments.minimum_hours}-hour public prerelease review gate")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
