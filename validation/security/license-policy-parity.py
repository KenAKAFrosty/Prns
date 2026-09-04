#!/usr/bin/env python3
"""Keep cargo-deny and cargo-about's repository-wide SPDX allowlists identical."""

from __future__ import annotations

from pathlib import Path
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[2]


def accepted_licenses(path: Path, key: str) -> set[str]:
    with path.open("rb") as source:
        document = tomllib.load(source)
    value = document
    for segment in key.split("."):
        value = value[segment]
    return set(value)


def main() -> int:
    deny = accepted_licenses(ROOT / "deny.toml", "licenses.allow")
    notices = accepted_licenses(ROOT / "about.toml", "accepted")
    if deny == notices:
        print("LICENSE_POLICY_PARITY_OK")
        return 0

    for license_id in sorted(deny - notices):
        print(f"cargo-about does not accept cargo-deny license: {license_id}", file=sys.stderr)
    for license_id in sorted(notices - deny):
        print(f"cargo-deny does not accept cargo-about license: {license_id}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
