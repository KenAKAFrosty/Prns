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
        sys.argv[1],
        "--features",
        sys.argv[2],
        "--",
        *sys.argv[3:],
    ]
    result = subprocess.run(command, cwd=ROOT, check=False)
    if result.returncode == 0:
        print(
            f"Guide example succeeded. Next, inspect personal-rns/examples/{sys.argv[1]}.rs; it is intentionally small."
        )
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
