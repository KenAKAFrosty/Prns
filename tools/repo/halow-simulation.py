"""Replay one bounded HaLoW simulation case on the production transport."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", type=Path, required=True)
    args = parser.parse_args()
    source = args.case.resolve()
    with source.open(encoding="utf-8") as stream:
        case = json.load(stream)
    if case.get("version") != 1 or "scenario" not in case:
        parser.error("expected a version 1 HaLoW case; extract 'case' from an artifact")
    environment = dict(os.environ)
    environment["PRNS_HALOW_CASE"] = str(source)
    root = Path(__file__).resolve().parents[2]
    return subprocess.run(
        ["cargo", "test", "--locked", "-p", "prns-simulation", "--features", "controlled-time", "--test", "halow", "--", "--ignored", "--exact", "campaign::replay_case", "--nocapture"],
        cwd=root, env=environment, check=False,
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
