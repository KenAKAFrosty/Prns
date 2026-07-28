from __future__ import annotations

import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
COMMANDS = (
    (
        "cargo",
        "clippy",
        "--manifest-path",
        "prns-ffi/Cargo.toml",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ),
    (
        "cargo",
        "clippy",
        "--package",
        "hopspot-flash",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ),
    (
        "cargo",
        "clippy",
        "--manifest-path",
        "personal-hopspot/desktop/Cargo.toml",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ),
    (
        "cargo",
        "build",
        "--manifest-path",
        "personal-hopspot/desktop/Cargo.toml",
        "--locked",
    ),
)


def main() -> None:
    for command in COMMANDS:
        subprocess.run(command, cwd=ROOT, check=True)
    print("WINDOWS_DESKTOP_GATE_OK")


if __name__ == "__main__":
    main()
