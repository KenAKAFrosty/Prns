#!/usr/bin/env python3

import argparse
import json
import re
import shutil
import subprocess
import tarfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "prns-host" / "distribution" / "packages.json"
ARCHIVE_PATH = re.compile(r"^[A-Za-z0-9_.+-]+-[0-9][A-Za-z0-9_.+-]*/")


def run(*arguments, cwd=ROOT):
    subprocess.run(arguments, cwd=cwd, check=True)


def git(*arguments):
    return subprocess.check_output(
        ["git", *arguments],
        cwd=ROOT,
        text=True,
    ).strip()


def lock_snapshot():
    paths = git("ls-files", "*Cargo.lock").splitlines()
    return {
        ROOT / path: (ROOT / path).read_bytes()
        for path in paths
    }


def restore_locks(snapshot):
    for path, contents in snapshot.items():
        if not path.is_file() or path.read_bytes() != contents:
            path.write_bytes(contents)


def safe_members(archive):
    for member in archive.getmembers():
        path = member.name
        if (
            member.issym()
            or member.islnk()
            or not ARCHIVE_PATH.match(path)
            or Path(path).is_absolute()
            or ".." in Path(path).parts
        ):
            raise ValueError(f"unsafe crate archive member: {path}")
        yield member


def unpack(crate, destination):
    with tarfile.open(crate, mode="r:gz") as archive:
        members = list(safe_members(archive))
        archive.extractall(destination, members=members)
    roots = [path for path in destination.iterdir() if path.is_dir()]
    if len(roots) != 1:
        raise ValueError(f"{crate} did not contain exactly one package root")
    return roots[0]


def consumer_manifest(crates, version):
    patches = []
    for name, path in crates.items():
        escaped = path.as_posix().replace("\\", "\\\\").replace('"', '\\"')
        patches.append(f'{name} = {{ path = "{escaped}" }}')
    dependencies = []
    for name in sorted(crates):
        features = (
            ', features = ["tokio-host"]'
            if name == "personal-rns"
            else ""
        )
        dependencies.append(
            f'{name} = {{ version = "={version}", '
            f"default-features = false{features} }}"
        )
    return (
        "[package]\n"
        'name = "personal-rns-package-smoke"\n'
        'version = "0.0.0"\n'
        'edition = "2021"\n'
        "publish = false\n"
        "\n"
        "[workspace]\n"
        "\n"
        "[dependencies]\n"
        + "\n".join(dependencies)
        + "\n"
        "\n"
        "[patch.crates-io]\n"
        + "\n".join(patches)
        + "\n"
    )


def patch_config(crates):
    patches = []
    for crate in crates:
        path = (ROOT / crate["manifest"]).parent.resolve()
        escaped = path.as_posix().replace("\\", "\\\\").replace('"', '\\"')
        patches.append(f'"{crate["name"]}" = {{ path = "{escaped}" }}')
    return "[patch.crates-io]\n" + "\n".join(patches) + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--expected-sha")
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    commit = git("rev-parse", "HEAD")
    if args.expected_sha and commit != args.expected_sha:
        raise SystemExit("checked-out commit differs from --expected-sha")
    if not args.allow_dirty and git("status", "--porcelain"):
        raise SystemExit("Rust package staging requires a clean worktree")
    output = Path(args.output).resolve()
    if output.exists() and any(output.iterdir()):
        raise SystemExit(f"output directory is not empty: {output}")
    crates_output = output / "crates"
    builds = output / "build"
    unpacked = output / "unpacked"
    crates_output.mkdir(parents=True)
    builds.mkdir()
    unpacked.mkdir()
    catalog = json.loads(CATALOG.read_text())
    version = (ROOT / "VERSION").read_text().strip()
    crates = sorted(
        catalog["rustCrates"],
        key=lambda crate: (crate["order"], crate["name"]),
    )
    packaging_config = output / "packaging-config.toml"
    packaging_config.write_text(patch_config(crates))
    packaged = {}
    locks = lock_snapshot()
    try:
        for crate in crates:
            manifest = ROOT / crate["manifest"]
            target = builds / crate["name"]
            run(
                "cargo",
                "package",
                "--manifest-path",
                str(manifest),
                "--no-verify",
                "--allow-dirty",
                "--config",
                str(packaging_config),
                "--target-dir",
                str(target),
            )
            archive = (
                target / "package" / f"{crate['name']}-{version}.crate"
            )
            if not archive.is_file():
                raise ValueError(f"cargo did not create {archive}")
            destination = crates_output / archive.name
            shutil.copy2(archive, destination)
            package_root = unpack(destination, unpacked / crate["name"])
            packaged[crate["name"]] = package_root
    finally:
        restore_locks(locks)
    consumer = output / "consumer"
    source = consumer / "src"
    source.mkdir(parents=True)
    (consumer / "Cargo.toml").write_text(
        consumer_manifest(packaged, version)
    )
    (source / "lib.rs").write_text("pub fn package_smoke() {}\n")
    run(
        "cargo",
        "check",
        "--manifest-path",
        str(consumer / "Cargo.toml"),
    )
    manifest = {
        "format": 1,
        "version": version,
        "commit": commit,
        "publicationOrder": [
            {
                "name": crate["name"],
                "order": crate["order"],
                "file": f"crates/{crate['name']}-{version}.crate",
            }
            for crate in crates
        ],
    }
    (output / "rust-crates.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    )


if __name__ == "__main__":
    main()
