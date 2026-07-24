#!/usr/bin/env python3

import argparse
import hashlib
import json
import shutil
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git(*args):
    return subprocess.check_output(
        ["git", *args],
        cwd=ROOT,
        text=True,
    ).strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--library", action="append", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--expected-sha")
    args = parser.parse_args()

    version = (ROOT / "VERSION").read_text().strip()
    catalog = json.loads(
        (ROOT / "prns-host/distribution/packages.json").read_text()
    )
    targets = {
        target["rust"]: target for target in catalog["nativeTargets"]
    }
    if args.target not in targets:
        raise SystemExit(f"unknown native target: {args.target}")
    expected_libraries = {
        targets[args.target]["dynamicLibrary"],
        targets[args.target]["staticLibrary"],
    }
    actual_libraries = {Path(value).name for value in args.library}
    if actual_libraries != expected_libraries:
        raise SystemExit(
            f"{args.target} requires libraries {sorted(expected_libraries)}, "
            f"got {sorted(actual_libraries)}"
        )
    schema = json.loads(
        (ROOT / "prns-host/schema/host-contract-v1.json").read_text()
    )
    if schema["productVersion"] != version:
        raise SystemExit("VERSION and host contract productVersion disagree")
    commit = git("rev-parse", "HEAD")
    if args.expected_sha and args.expected_sha != commit:
        raise SystemExit("the checked-out commit does not match --expected-sha")

    output = Path(args.output).resolve()
    if output.exists() and any(output.iterdir()):
        raise SystemExit(f"output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    include = output / "include"
    lib = output / "lib"
    include.mkdir(exist_ok=True)
    lib.mkdir(exist_ok=True)
    header = include / "prns_host.h"
    shutil.copy2(
        ROOT / "prns-host/abi/c/include/prns_host.h",
        header,
    )
    license_apache = output / "LICENSE-APACHE"
    license_mit = output / "LICENSE-MIT"
    package_readme = output / "PACKAGE.md"
    shutil.copy2(ROOT / "LICENSE-APACHE", license_apache)
    shutil.copy2(ROOT / "LICENSE-MIT", license_mit)
    shutil.copy2(
        ROOT / "prns-host" / "distribution" / "PACKAGE.md",
        package_readme,
    )
    pkgconfig = lib / "pkgconfig"
    pkgconfig.mkdir(exist_ok=True)
    pkgconfig_file = pkgconfig / "personal-rns.pc"
    pkgconfig_file.write_text(
        "prefix=${pcfiledir}/../..\n"
        "includedir=${prefix}/include\n"
        "libdir=${prefix}/lib\n"
        "\n"
        "Name: personal-rns\n"
        "Description: Personal Reticulum stable host ABI\n"
        f"Version: {version}\n"
        "Libs: -L${libdir} -lprns_host\n"
        "Cflags: -I${includedir}\n"
    )
    packaged = [
        header,
        license_apache,
        license_mit,
        package_readme,
        pkgconfig_file,
    ]
    library_names = set()
    for value in args.library:
        source = Path(value).resolve()
        if not source.is_file():
            raise SystemExit(f"native library does not exist: {source}")
        if source.name in library_names:
            raise SystemExit(f"duplicate native library name: {source.name}")
        library_names.add(source.name)
        destination = lib / source.name
        shutil.copy2(source, destination)
        packaged.append(destination)
    assets = []
    for path in packaged:
        assets.append(
            {
                "path": path.relative_to(output).as_posix(),
                "sha256": sha256(path),
                "bytes": path.stat().st_size,
            }
        )
    assets.sort(key=lambda item: item["path"])
    manifest = {
        "format": 1,
        "product": "personal-rns",
        "version": version,
        "contractAbi": schema["abi"],
        "schemaVersion": schema["schemaVersion"],
        "target": args.target,
        "commit": commit,
        "assets": assets,
    }
    (output / "artifact.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    )


if __name__ == "__main__":
    main()
