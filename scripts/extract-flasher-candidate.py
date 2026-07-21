#!/usr/bin/env python3
"""Extract a signed-candidate archive without accepting links or escaping paths."""

from __future__ import annotations

import argparse
from pathlib import Path, PurePosixPath
import tarfile


MAX_FILES = 100_000
MAX_BYTES = 2 * 1024 * 1024 * 1024


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("archive", type=Path)
    parser.add_argument("destination", type=Path)
    arguments = parser.parse_args()
    with tarfile.open(arguments.archive, "r:gz") as archive:
        members = archive.getmembers()
        if len(members) > MAX_FILES or sum(member.size for member in members) > MAX_BYTES:
            parser.error("candidate archive exceeds the release extraction limits")
        for member in members:
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or member.issym() or member.islnk():
                parser.error(f"unsafe candidate archive member: {member.name}")
            if not member.isfile() and not member.isdir():
                parser.error(f"unsupported candidate archive member: {member.name}")
        arguments.destination.mkdir(parents=True, exist_ok=True)
        archive.extractall(arguments.destination, members=members, filter="data")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
