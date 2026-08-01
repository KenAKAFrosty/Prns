#!/usr/bin/env python3

import argparse
import json
import os
import subprocess
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "prns-host" / "distribution" / "packages.json"


def git(*arguments):
    return subprocess.check_output(
        ["git", *arguments],
        cwd=ROOT,
        text=True,
    ).strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--expected-sha", required=True)
    args = parser.parse_args()
    commit = git("rev-parse", "HEAD")
    if commit != args.expected_sha:
        raise SystemExit("checked-out commit differs from --expected-sha")
    if git("status", "--porcelain"):
        raise SystemExit("Rust publication requires a clean worktree")
    if not os.environ.get("CARGO_REGISTRY_TOKEN"):
        raise SystemExit("CARGO_REGISTRY_TOKEN is required")
    catalog = json.loads(CATALOG.read_text())
    version = (ROOT / "VERSION").read_text().strip()
    crates = sorted(
        catalog["rustCrates"],
        key=lambda crate: (crate["order"], crate["name"]),
    )
    for index, crate in enumerate(crates):
        subprocess.run(
            [
                "cargo",
                "publish",
                "--manifest-path",
                str(ROOT / crate["manifest"]),
                "--locked",
            ],
            cwd=ROOT,
            check=True,
        )
        if index == len(crates) - 1:
            continue
        package = f"{crate['name']}@{version}"
        for attempt in range(18):
            available = subprocess.run(
                ["cargo", "info", "--registry", "crates-io", package],
                cwd=ROOT,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            if available.returncode == 0:
                break
            if attempt == 17:
                raise SystemExit(
                    f"published crate did not become available: {package}"
                )
            time.sleep(10)


if __name__ == "__main__":
    main()
