#!/usr/bin/env python3
"""Require a real five-architecture tester roster before candidate signing."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from flasher_acceptance_contract import CLI_TARGETS, SHIPPING_BOARDS  # noqa: E402,F401
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
    print("tester roster covers all five published host architectures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
