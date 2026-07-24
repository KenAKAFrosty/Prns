#!/usr/bin/env python3

import argparse
import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "prns-host" / "distribution" / "packages.json"
LABEL = re.compile(r"^[a-z0-9][a-z0-9._-]*$")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as contents:
        for chunk in iter(lambda: contents.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git(*arguments):
    return subprocess.check_output(
        ["git", *arguments],
        cwd=ROOT,
        text=True,
    ).strip()


def copy_artifact(label, source, destination):
    target = destination / label
    if source.is_file():
        target.mkdir(parents=True)
        shutil.copy2(source, target / source.name)
        return
    if source.is_dir():
        shutil.copytree(source, target)
        return
    raise ValueError(f"artifact does not exist: {source}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact", action="append", default=[])
    parser.add_argument("--output", required=True)
    parser.add_argument("--expected-sha", required=True)
    args = parser.parse_args()
    commit = git("rev-parse", "HEAD")
    if commit != args.expected_sha:
        raise SystemExit("checked-out commit differs from --expected-sha")
    output = Path(args.output).resolve()
    if output.exists() and any(output.iterdir()):
        raise SystemExit(f"output directory is not empty: {output}")
    artifacts = output / "artifacts"
    artifacts.mkdir(parents=True, exist_ok=True)
    labels = set()
    for value in args.artifact:
        label, separator, raw_path = value.partition("=")
        if not separator or not LABEL.fullmatch(label) or label in labels:
            raise SystemExit(f"invalid or duplicate artifact: {value}")
        labels.add(label)
        copy_artifact(label, Path(raw_path).resolve(), artifacts)
    catalog = json.loads(CATALOG.read_text())
    version = (ROOT / "VERSION").read_text().strip()
    schema = json.loads((ROOT / catalog["contractSource"]).read_text())
    files = []
    for path in sorted(artifacts.rglob("*")):
        if not path.is_file():
            continue
        files.append(
            {
                "path": path.relative_to(output).as_posix(),
                "bytes": path.stat().st_size,
                "sha256": sha256(path),
            }
        )
    resolved_packages = []
    for package in catalog["packages"]:
        resolved = dict(package)
        if "tag" in resolved:
            resolved["tag"] = resolved["tag"].format(version=version)
        resolved_packages.append(resolved)
    release_index = {
        "format": 1,
        "product": catalog["product"],
        "version": version,
        "commit": commit,
        "contractAbi": schema["abi"],
        "schemaVersion": schema["schemaVersion"],
        "packages": resolved_packages,
        "rustCrates": catalog["rustCrates"],
        "nativeTargets": catalog["nativeTargets"],
        "files": files,
    }
    (output / "release-index.json").write_text(
        json.dumps(release_index, indent=2, sort_keys=True) + "\n"
    )
    shutil.copy2(
        ROOT / "prns-host" / "distribution" / "PACKAGE.md",
        output / "PACKAGE.md",
    )
    shutil.copy2(
        ROOT / "prns-host" / "distribution" / "ADMIN.md",
        output / "ADMIN.md",
    )
    shutil.copy2(ROOT / "LICENSE-APACHE", output / "LICENSE-APACHE")
    shutil.copy2(ROOT / "LICENSE-MIT", output / "LICENSE-MIT")


if __name__ == "__main__":
    main()
