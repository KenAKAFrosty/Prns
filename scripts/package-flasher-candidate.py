#!/usr/bin/env python3
"""Create a deterministic, link-free flasher candidate archive."""

from __future__ import annotations

import argparse
import gzip
from pathlib import Path
import stat
import sys
import tarfile


def archive_paths(root: Path) -> list[Path]:
    paths: list[Path] = []
    for path in root.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"candidate cannot contain symlink {path}")
        if not path.is_file() and not path.is_dir():
            raise ValueError(f"candidate contains unsupported filesystem entry {path}")
        paths.append(path)
    return sorted(paths, key=lambda path: path.relative_to(root).as_posix())


def normalized_mode(path: Path) -> int:
    if path.is_dir():
        return 0o755
    return 0o755 if path.stat().st_mode & stat.S_IXUSR else 0o644


def package(root: Path, output: Path) -> None:
    if not root.is_dir():
        raise ValueError(f"candidate directory does not exist: {root}")
    resolved_root = root.resolve()
    resolved_output = output.resolve()
    try:
        resolved_output.relative_to(resolved_root)
    except ValueError:
        pass
    else:
        raise ValueError("candidate archive cannot be created inside the candidate directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f".{output.name}.tmp")
    if temporary.exists():
        temporary.unlink()
    try:
        with temporary.open("wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                    for path in archive_paths(resolved_root):
                        relative = path.relative_to(resolved_root).as_posix()
                        info = archive.gettarinfo(str(path), arcname=relative)
                        info.uid = 0
                        info.gid = 0
                        info.uname = "root"
                        info.gname = "root"
                        info.mtime = 0
                        info.mode = normalized_mode(path)
                        info.pax_headers = {}
                        if path.is_file():
                            with path.open("rb") as stream:
                                archive.addfile(info, stream)
                        else:
                            archive.addfile(info)
        temporary.replace(output)
    finally:
        if temporary.exists():
            temporary.unlink()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("candidate", type=Path)
    parser.add_argument("output", type=Path)
    arguments = parser.parse_args()
    try:
        package(arguments.candidate, arguments.output)
    except (OSError, ValueError) as error:
        print(f"candidate packaging failed: {error}", file=sys.stderr)
        return 1
    print(arguments.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
