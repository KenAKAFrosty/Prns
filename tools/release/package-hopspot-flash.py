#!/usr/bin/env python3
"""Create one deterministic, self-contained hopspot-flash release archive."""

from __future__ import annotations

import argparse
import gzip
import os
from pathlib import Path
import shutil
import stat
import tarfile
import zipfile


ROOT = Path(__file__).resolve().parents[2]
TARGET_EXTENSIONS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-apple-darwin": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
}


def add_tar_file(archive: tarfile.TarFile, path: Path, name: str, mode: int) -> None:
    info = archive.gettarinfo(str(path), arcname=name)
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    info.mtime = 0
    info.mode = mode
    with path.open("rb") as stream:
        archive.addfile(info, stream)


def write_tar(output: Path, files: list[tuple[Path, str, int]]) -> None:
    with output.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as archive:
                for path, name, mode in files:
                    add_tar_file(archive, path, name, mode)


def write_zip(output: Path, files: list[tuple[Path, str, int]]) -> None:
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for path, name, mode in files:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (mode & 0xFFFF) << 16
            archive.writestr(info, path.read_bytes())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--target", choices=sorted(TARGET_EXTENSIONS), required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    arguments = parser.parse_args()

    version = os.environ.get("PRNS_FLASH_VERSION")
    if version is None:
        version = (ROOT / "VERSION").read_text(encoding="utf-8").strip()
    else:
        version = version.strip()
    if not version or version.lower() == "next":
        parser.error("flasher release version is not publishable")
    if any(
        character
        not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-+"
        for character in version
    ):
        parser.error("flasher release version is not path-safe")
    binary = arguments.binary.resolve()
    if not binary.is_file():
        parser.error(f"binary does not exist: {binary}")
    output_dir = arguments.out_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    extension = TARGET_EXTENSIONS[arguments.target]
    output = output_dir / f"hopspot-flash-{version}-{arguments.target}{extension}"
    executable_name = "hopspot-flash.exe" if extension == ".zip" else "hopspot-flash"
    files = [
        (binary, executable_name, stat.S_IFREG | 0o755),
        (ROOT / "LICENSE-APACHE", "LICENSE-APACHE", stat.S_IFREG | 0o644),
        (ROOT / "LICENSE-MIT", "LICENSE-MIT", stat.S_IFREG | 0o644),
        (ROOT / "THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.md", stat.S_IFREG | 0o644),
    ]
    temporary = output.with_suffix(output.suffix + f".part-{os.getpid()}")
    if extension == ".zip":
        write_zip(temporary, files)
    else:
        write_tar(temporary, files)
    shutil.move(temporary, output)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
