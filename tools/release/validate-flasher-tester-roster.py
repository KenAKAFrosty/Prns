#!/usr/bin/env python3
"""Require exact physical, fallback, and installer assignments before signing."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from flasher_acceptance_contract import (  # noqa: E402,F401
    CLI_TARGETS,
    OS_ARCHITECTURES,
    REQUIRED_FALLBACKS,
    SHIPPING_BOARDS,
    SURFACES,
)
from flasher_tester_roster import validate_roster  # noqa: E402


def validate(
    roster: object,
    expected_version: str,
) -> list[str]:
    _, errors = validate_roster(roster, expected_version)
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--roster", type=Path, required=True)
    parser.add_argument("--version", required=True)
    arguments = parser.parse_args()
    if not arguments.version or arguments.version.lower() == "next":
        parser.error("an immutable candidate version is required")
    try:
        roster = json.loads(arguments.roster.read_text(encoding="utf-8"))
        errors = validate(roster, arguments.version)
    except (OSError, json.JSONDecodeError) as error:
        print(f"tester roster validation failed: {error}", file=sys.stderr)
        return 1
    if errors:
        for error in errors:
            print(f"tester roster validation failed: {error}", file=sys.stderr)
        return 1
    print(
        f"tester roster covers {len(SHIPPING_BOARDS) * len(SURFACES)} physical, "
        f"{len(REQUIRED_FALLBACKS)} fallback, and "
        f"{len(OS_ARCHITECTURES)} installer assignments"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
