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
        "hopspot-flash",
        "--",
        "flash",
        "--local-build",
        *sys.argv[1:],
    ]
    try:
        return subprocess.run(command, cwd=ROOT, check=False).returncode
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
