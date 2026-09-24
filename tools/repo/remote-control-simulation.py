"""Repository entry point for one isolated Remote Control simulator case."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("replay", "reduce"), required=True)
    parser.add_argument("--case", type=Path, required=True)
    args = parser.parse_args()
    source = args.case.resolve()
    with source.open(encoding="utf-8") as stream:
        document = json.load(stream)
    if document.get("version") != 1:
        parser.error("expected a version 1 simulator case or artifact")
    root = Path(__file__).resolve().parents[2]
    environment = dict(os.environ)
    environment["PRNS_REMOTE_CONTROL_SIM_CASE"] = str(source)
    test = f"remote_control::campaign::subprocess::{args.mode}_case"
    return subprocess.run(
        ["cargo", "test", "--locked", "-p", "prns-simulation", "--features", "controlled-time", "--test", "embassy_ble", "--", "--ignored", "--exact", test, "--nocapture"],
        cwd=root,
        env=environment,
        check=False,
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
