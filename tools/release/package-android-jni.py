#!/usr/bin/env python3

import argparse
import hashlib
import json
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
TARGETS = {
    "aarch64-linux-android": "arm64-v8a",
    "armv7-linux-androideabi": "armeabi-v7a",
}


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as contents:
        for chunk in iter(lambda: contents.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def add_file(archive, source, destination):
    info = zipfile.ZipInfo(destination, date_time=(1980, 1, 1, 0, 0, 0))
    info.external_attr = (source.stat().st_mode & 0o777) << 16
    info.compress_type = zipfile.ZIP_DEFLATED
    with source.open("rb") as contents:
        archive.writestr(info, contents.read(), compresslevel=9)


def add_bytes(archive, contents, destination):
    info = zipfile.ZipInfo(destination, date_time=(1980, 1, 1, 0, 0, 0))
    info.external_attr = 0o644 << 16
    info.compress_type = zipfile.ZIP_DEFLATED
    archive.writestr(info, contents, compresslevel=9)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--native", action="append", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    provided = {}
    for value in args.native:
        target, separator, raw_path = value.partition("=")
        if not separator or target not in TARGETS or target in provided:
            raise SystemExit(f"invalid or duplicate Android native input: {value}")
        path = Path(raw_path).resolve()
        if path.is_dir():
            path = path / "lib" / "libprns_host.so"
        if not path.is_file():
            raise SystemExit(f"Android native library does not exist: {path}")
        provided[target] = path
    if provided.keys() != TARGETS.keys():
        raise SystemExit("both Android native targets are required")
    output = Path(args.output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    manifest = {
        "format": 1,
        "version": (ROOT / "VERSION").read_text().strip(),
        "libraries": [
            {
                "target": target,
                "abi": TARGETS[target],
                "sha256": sha256(provided[target]),
            }
            for target in sorted(provided)
        ],
    }
    manifest_bytes = (
        json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    ).encode()
    with zipfile.ZipFile(
        output,
        mode="w",
        compression=zipfile.ZIP_DEFLATED,
        compresslevel=9,
    ) as archive:
        for target in sorted(provided):
            add_file(
                archive,
                provided[target],
                f"jniLibs/{TARGETS[target]}/libprns_host.so",
            )
        add_file(
            archive,
            ROOT / "prns-host" / "distribution" / "PACKAGE.md",
            "PACKAGE.md",
        )
        add_file(archive, ROOT / "LICENSE-APACHE", "LICENSE-APACHE")
        add_file(archive, ROOT / "LICENSE-MIT", "LICENSE-MIT")
        add_bytes(archive, manifest_bytes, "android-jni.json")


if __name__ == "__main__":
    main()
