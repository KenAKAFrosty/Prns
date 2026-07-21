#!/usr/bin/env python3
"""Capture release-candidate provenance without including environment variables or secrets."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import platform
import subprocess


def output(*command: str) -> str:
    process = subprocess.run(command, text=True, capture_output=True, check=False)
    return (process.stdout or process.stderr).strip().splitlines()[0] if (process.stdout or process.stderr).strip() else "unavailable"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    arguments = parser.parse_args()
    metadata = {
        "schema": 1,
        "source_commit": arguments.commit,
        "built_at_utc": datetime.now(timezone.utc).replace(microsecond=0).isoformat(),
        "host": {"system": platform.system(), "machine": platform.machine()},
        "tools": {
            "rustc": output("rustc", "--version"),
            "cargo": output("cargo", "--version"),
            "node": output("node", "--version"),
            "npm": output("npm", "--version"),
            "dioxus": output("dx", "--version"),
            "git": output("git", "--version"),
        },
    }
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
