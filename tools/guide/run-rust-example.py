#!/usr/bin/env python3
"""Run the canonical Rust learning example with its complete feature contract."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def main() -> int:
    command = [
        "cargo",
        "run",
        "--locked",
        "-p",
        "personal-rns",
        "--example",
        "node_basics",
        "--features",
        "tokio-host,tcp,wifi-auto,usb,bluetooth-auto",
        "--",
        *sys.argv[1:],
    ]
    return subprocess.run(command, cwd=ROOT, check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main())
