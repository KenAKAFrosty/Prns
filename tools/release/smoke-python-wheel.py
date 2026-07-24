#!/usr/bin/env python3

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--wheel-dir", required=True)
    args = parser.parse_args()
    wheel_dir = Path(args.wheel_dir).resolve()
    wheels = sorted(wheel_dir.glob("*.whl"))
    if len(wheels) != 1:
        raise SystemExit(
            f"expected exactly one wheel in {wheel_dir}, found {len(wheels)}"
        )
    with tempfile.TemporaryDirectory(prefix="prns-python-wheel-") as temporary:
        target = Path(temporary) / "site"
        subprocess.run(
            [
                sys.executable,
                "-m",
                "pip",
                "install",
                "--no-deps",
                "--target",
                str(target),
                str(wheels[0]),
            ],
            check=True,
        )
        environment = os.environ.copy()
        environment.pop("PRNS_HOST_LIBRARY", None)
        environment["PYTHONPATH"] = str(target)
        subprocess.run(
            [
                sys.executable,
                str(ROOT / "prns-host/bindings/python/tests/smoke.py"),
            ],
            cwd=ROOT,
            env=environment,
            check=True,
        )


if __name__ == "__main__":
    main()
