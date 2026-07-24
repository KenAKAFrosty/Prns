#!/usr/bin/env python3
"""Package the exact website release commit as a deterministic source ZIP."""

from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys
import zipfile

from source_snapshot import package_source_snapshot


ROOT = Path(__file__).resolve().parents[2]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", type=Path, default=ROOT)
    parser.add_argument("--commit")
    parser.add_argument("--version")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--checksum", type=Path)
    parser.add_argument("--metadata", type=Path)
    arguments = parser.parse_args()
    try:
        commit = arguments.commit
        if commit is None:
            result = subprocess.run(
                ("git", "-C", str(arguments.repository), "rev-parse", "HEAD"),
                text=True,
                capture_output=True,
                check=False,
            )
            if result.returncode != 0:
                raise ValueError("could not resolve HEAD for the source snapshot")
            commit = result.stdout.strip()
        version = arguments.version
        if version is None:
            version = (arguments.repository / "VERSION").read_text(encoding="utf-8").strip()
        archive, checksum = package_source_snapshot(
            repository=arguments.repository,
            commit=commit,
            version=version,
            output=arguments.output,
            checksum=arguments.checksum,
            metadata=arguments.metadata,
        )
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"source snapshot packaging failed: {error}", file=sys.stderr)
        return 1
    print(f"packaged source snapshot {commit} at {archive}")
    print(f"wrote source snapshot checksum at {checksum}")
    if arguments.metadata is not None:
        print(f"wrote canonical source metadata at {arguments.metadata.resolve()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
