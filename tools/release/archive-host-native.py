#!/usr/bin/env python3

import argparse
import gzip
import json
import tarfile
import zipfile
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "prns-host" / "distribution" / "packages.json"


def catalog_target(rust_target):
    catalog = json.loads(CATALOG.read_text())
    matches = [
        target
        for target in catalog["nativeTargets"]
        if target["rust"] == rust_target
    ]
    if len(matches) != 1:
        raise ValueError(f"unknown native target {rust_target}")
    return matches[0]


def files_in(source):
    for path in sorted(source.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"native artifact contains a symlink: {path}")
        if path.is_file():
            yield path


def tar_archive(source, output, prefix):
    with output.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(
                mode="w",
                fileobj=compressed,
                format=tarfile.PAX_FORMAT,
            ) as archive:
                for path in files_in(source):
                    relative = path.relative_to(source).as_posix()
                    info = archive.gettarinfo(
                        str(path),
                        arcname=str(PurePosixPath(prefix, relative)),
                    )
                    info.uid = 0
                    info.gid = 0
                    info.uname = ""
                    info.gname = ""
                    info.mtime = 0
                    with path.open("rb") as contents:
                        archive.addfile(info, contents)


def zip_archive(source, output, prefix):
    with zipfile.ZipFile(
        output,
        mode="w",
        compression=zipfile.ZIP_DEFLATED,
        compresslevel=9,
    ) as archive:
        for path in files_in(source):
            relative = path.relative_to(source).as_posix()
            info = zipfile.ZipInfo(
                str(PurePosixPath(prefix, relative)),
                date_time=(1980, 1, 1, 0, 0, 0),
            )
            mode = path.stat().st_mode & 0o777
            info.external_attr = mode << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            with path.open("rb") as contents:
                archive.writestr(info, contents.read(), compresslevel=9)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    source = Path(args.source).resolve()
    output = Path(args.output).resolve()
    if not source.is_dir() or not (source / "artifact.json").is_file():
        raise SystemExit("source must be a packaged native artifact")
    target = catalog_target(args.target)
    artifact = json.loads((source / "artifact.json").read_text())
    if artifact["target"] != args.target:
        raise SystemExit("native artifact target differs from --target")
    for library in (target["dynamicLibrary"], target["staticLibrary"]):
        if not (source / "lib" / library).is_file():
            raise SystemExit(f"native artifact is missing lib/{library}")
    expected_suffix = f".{target['archive']}"
    if not output.name.endswith(expected_suffix):
        raise SystemExit(f"output must end in {expected_suffix}")
    output.parent.mkdir(parents=True, exist_ok=True)
    prefix = f"personal-rns-{artifact['version']}-{args.target}"
    if target["archive"] == "tar.gz":
        tar_archive(source, output, prefix)
    else:
        zip_archive(source, output, prefix)


if __name__ == "__main__":
    main()
