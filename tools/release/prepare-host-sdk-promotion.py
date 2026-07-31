#!/usr/bin/env python3

import argparse
import hashlib
import json
import shutil
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "prns-host" / "distribution" / "packages.json"


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as contents:
        for chunk in iter(lambda: contents.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def one_indexed_path(entries, name):
    matches = [path for path in entries if Path(path).name == name]
    if len(matches) != 1:
        raise ValueError(f"expected one staged {name}, found {len(matches)}")
    return matches[0]


def copy_unique(source, output, names):
    if source.name in names:
        raise ValueError(f"promotion asset name collides: {source.name}")
    names.add(source.name)
    target = output / source.name
    shutil.copy2(source, target)
    return target


def prepare(stage, output, expected_sha, root=ROOT):
    stage = Path(stage).resolve()
    output = Path(output).resolve()
    if output.exists() and any(output.iterdir()):
        raise ValueError(f"output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    catalog = json.loads((root / "prns-host/distribution/packages.json").read_text())
    version = (root / "VERSION").read_text().strip()
    index = json.loads((stage / "release-index.json").read_text())
    if index["commit"] != expected_sha:
        raise ValueError("release stage commit differs from --expected-sha")
    if index["version"] != version:
        raise ValueError("release stage version differs from VERSION")
    resolved_packages = []
    for package in catalog["packages"]:
        resolved = dict(package)
        if "tag" in resolved:
            resolved["tag"] = resolved["tag"].format(version=version)
        resolved_packages.append(resolved)
    if index["packages"] != resolved_packages:
        raise ValueError("release stage package custody differs from the catalog")
    entries = {entry["path"]: entry for entry in index["files"]}
    if len(entries) != len(index["files"]):
        raise ValueError("release stage contains duplicate indexed paths")
    required = []
    for target in catalog["nativeTargets"]:
        suffix = target["archive"]
        required.append(f"personal-rns-{version}-{target['rust']}.{suffix}")
    required.extend(
        [
            f"personal-rns-{version}-go.tar.gz",
            f"personal-rns-{version}-swift.tar.gz",
            f"personal-rns-{version}-julia.tar.gz",
            f"personal-rns-{version}-android-jni.zip",
            "source-sdks.json",
        ]
    )
    copied = []
    names = set()
    for name in required:
        relative = one_indexed_path(entries, name)
        source = stage / relative
        entry = entries[relative]
        if not source.is_file():
            raise ValueError(f"indexed promotion asset is missing: {relative}")
        if source.stat().st_size != entry["bytes"] or sha256(source) != entry["sha256"]:
            raise ValueError(f"indexed promotion asset changed: {relative}")
        copied.append(copy_unique(source, output, names))
    for name in (
        "ADMIN.md",
        "LICENSE-APACHE",
        "LICENSE-MIT",
        "PACKAGE.md",
        "release-index.json",
    ):
        source = stage / name
        if not source.is_file():
            raise ValueError(f"release stage is missing {name}")
        copied.append(copy_unique(source, output, names))
    source_manifest = json.loads((output / "source-sdks.json").read_text())
    expected_source_tags = {
        package["ecosystem"]: package["tag"]
        for package in resolved_packages
        if package["ecosystem"] in {"go", "julia", "swift"}
    }
    if source_manifest != {
        "format": 1,
        "version": version,
        "commit": expected_sha,
        "tags": expected_source_tags,
    }:
        raise ValueError("source SDK manifest differs from release custody")
    tags = sorted(
        {
            package["tag"]
            for package in index["packages"]
            if "tag" in package
        }
    )
    release_tag = f"host-sdk-v{version}"
    if release_tag not in tags:
        raise ValueError("release stage lacks C and C++ tag custody")
    promotion = {
        "format": 1,
        "product": index["product"],
        "version": version,
        "commit": expected_sha,
        "releaseTag": release_tag,
        "tags": tags,
        "assets": [
            {
                "name": path.name,
                "bytes": path.stat().st_size,
                "sha256": sha256(path),
            }
            for path in sorted(copied)
        ],
    }
    promotion_path = output / "promotion.json"
    promotion_path.write_text(json.dumps(promotion, indent=2, sort_keys=True) + "\n")
    checksum_paths = sorted([*copied, promotion_path], key=lambda path: path.name)
    (output / "SHA256SUMS").write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in checksum_paths)
    )
    return promotion


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--expected-sha", required=True)
    args = parser.parse_args()
    try:
        promotion = prepare(args.stage, args.output, args.expected_sha)
    except (KeyError, OSError, ValueError, json.JSONDecodeError) as error:
        raise SystemExit(str(error)) from error
    print(
        f"HOST_SDK_PROMOTION_READY version={promotion['version']} "
        f"assets={len(promotion['assets'])} tags={len(promotion['tags'])}"
    )


if __name__ == "__main__":
    main()
