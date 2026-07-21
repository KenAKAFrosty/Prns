#!/usr/bin/env python3
"""Package reviewed qualification objects into a deterministic flat archive."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import os
from pathlib import Path
import re
import sys
import tarfile
import tempfile


SHA256_NAME = re.compile(r"^[0-9a-f]{64}$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def evidence_objects(root: Path) -> list[Path]:
    if root.is_symlink() or not root.is_dir():
        raise ValueError("qualification evidence root must be a real directory")
    objects = sorted(root.iterdir(), key=lambda path: path.name)
    if not objects:
        raise ValueError("qualification evidence root is empty")
    for path in objects:
        if (
            SHA256_NAME.fullmatch(path.name) is None
            or path.is_symlink()
            or not path.is_file()
        ):
            raise ValueError(
                "qualification evidence root may contain only regular files named by lowercase SHA-256"
            )
        if path.stat().st_size == 0:
            raise ValueError(f"qualification evidence object is empty: {path.name}")
        if sha256(path) != path.name:
            raise ValueError(f"qualification evidence object name differs from its bytes: {path.name}")
    return objects


def package(root: Path, output: Path) -> None:
    objects = evidence_objects(root)
    if output.exists():
        raise ValueError(f"refusing to overwrite qualification evidence archive: {output}")
    resolved_root = root.resolve()
    try:
        output.resolve().relative_to(resolved_root)
    except ValueError:
        pass
    else:
        raise ValueError("qualification evidence archive cannot be created inside its source root")
    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{output.name}.", suffix=".tmp", dir=output.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                    for path in objects:
                        info = archive.gettarinfo(str(path), arcname=path.name)
                        info.uid = 0
                        info.gid = 0
                        info.uname = "root"
                        info.gname = "root"
                        info.mtime = 0
                        info.mode = 0o644
                        info.pax_headers = {}
                        with path.open("rb") as stream:
                            archive.addfile(info, stream)
        os.replace(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("evidence_root", type=Path)
    parser.add_argument("output", type=Path)
    arguments = parser.parse_args()
    try:
        package(arguments.evidence_root, arguments.output)
    except (OSError, ValueError) as error:
        print(f"qualification evidence packaging failed: {error}", file=sys.stderr)
        return 1
    print(f"{sha256(arguments.output)}  {arguments.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
