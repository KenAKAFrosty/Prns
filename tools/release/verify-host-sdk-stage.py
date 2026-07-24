#!/usr/bin/env python3

import argparse
import hashlib
import json
import re
import tarfile
import zipfile
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[2]
ALLOWED_HOME_USERS = {"op", "operator", "prns", "user"}
UNIX_HOME = re.compile(rb"/(?:home|Users)/([A-Za-z0-9_.-]+)")
WINDOWS_HOME = re.compile(
    rb"(?i)[a-z]:[\\/]+users[\\/]+([A-Za-z0-9_.-]+)"
)
SECRET_PATTERNS = {
    "GitHub token": re.compile(rb"\bgh[pousr]_[A-Za-z0-9]{20,}\b"),
    "npm token": re.compile(rb"\bnpm_[A-Za-z0-9]{36}\b"),
    "OpenPGP or PEM private key": re.compile(
        rb"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----"
    ),
    "PyPI token": re.compile(rb"\bpypi-[A-Za-z0-9_-]{50,}\b"),
}
FORBIDDEN_PARTS = {
    ".git",
    ".gradle",
    "__pycache__",
    "node_modules",
    "target",
}
FORBIDDEN_NAMES = {
    ".env",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "credentials",
    "credentials.toml",
    "gradle.properties",
    "id_dsa",
    "id_ed25519",
    "id_rsa",
    "local.properties",
    "secring.gpg",
    "settings.xml",
}


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as contents:
        for chunk in iter(lambda: contents.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def safe_archive_path(raw):
    if "\\" in raw:
        raise ValueError(f"archive member uses backslashes: {raw}")
    path = PurePosixPath(raw)
    if path.is_absolute() or ".." in path.parts:
        raise ValueError(f"unsafe archive member: {raw}")
    if FORBIDDEN_PARTS.intersection(path.parts):
        raise ValueError(f"build or repository state in archive: {raw}")
    if path.name.lower() in FORBIDDEN_NAMES:
        raise ValueError(f"secret-bearing filename in archive: {raw}")


def scan_text(label, contents):
    for pattern in (UNIX_HOME, WINDOWS_HOME):
        for match in pattern.finditer(contents):
            user = match.group(1).decode("ascii").lower()
            if user not in ALLOWED_HOME_USERS:
                path = match.group(0).decode("ascii")
                raise ValueError(f"personal path in {label}: {path}")
    for secret_type, pattern in SECRET_PATTERNS.items():
        if pattern.search(contents):
            raise ValueError(f"{secret_type} in {label}")


def scan_tar(path):
    with tarfile.open(path, mode="r:*") as archive:
        for member in archive.getmembers():
            safe_archive_path(member.name)
            if member.issym() or member.islnk():
                raise ValueError(f"archive contains a link: {path}:{member.name}")
            if not (member.isfile() or member.isdir()):
                raise ValueError(
                    f"archive contains a special file: {path}:{member.name}"
                )
            if not member.isfile():
                continue
            contents = archive.extractfile(member)
            if contents is not None:
                scan_text(f"{path}:{member.name}", contents.read())


def scan_zip(path):
    with zipfile.ZipFile(path) as archive:
        for member in archive.infolist():
            safe_archive_path(member.filename)
            mode = member.external_attr >> 16
            file_type = mode & 0o170000
            if file_type not in {0, 0o040000, 0o100000}:
                raise ValueError(
                    f"archive contains a special file: {path}:{member.filename}"
                )
            if member.is_dir():
                continue
            scan_text(f"{path}:{member.filename}", archive.read(member))


def scan_artifact(path):
    name = path.name
    if (
        name.endswith(".tar.gz")
        or name.endswith(".tgz")
        or name.endswith(".crate")
    ):
        scan_tar(path)
    elif (
        name.endswith(".zip")
        or name.endswith(".whl")
        or name.endswith(".nupkg")
        or name.endswith(".snupkg")
        or name.endswith(".jar")
    ):
        scan_zip(path)
    else:
        scan_text(str(path), path.read_bytes())


def required_labels(catalog):
    labels = {
        "host-maven",
        "host-npm",
        "host-nuget",
        "host-rust",
        "host-sources",
    }
    for target in catalog["nativeTargets"]:
        labels.add(f"host-native-{target['rust']}")
        if "pythonPlatform" in target:
            labels.add(f"host-python-{target['rust']}")
    return labels


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage", required=True)
    parser.add_argument("--expected-sha", required=True)
    parser.add_argument("--allow-partial", action="store_true")
    args = parser.parse_args()
    stage = Path(args.stage).resolve()
    release_index_path = stage / "release-index.json"
    release_index = json.loads(release_index_path.read_text())
    version = (ROOT / "VERSION").read_text().strip()
    if release_index["commit"] != args.expected_sha:
        raise SystemExit("release index commit differs from --expected-sha")
    if release_index["version"] != version:
        raise SystemExit("release index version differs from VERSION")
    catalog = json.loads(
        (ROOT / "prns-host" / "distribution" / "packages.json").read_text()
    )
    artifacts = stage / "artifacts"
    actual_files = {
        path.relative_to(stage).as_posix()
        for path in artifacts.rglob("*")
        if path.is_file()
    }
    indexed_files = {entry["path"] for entry in release_index["files"]}
    if actual_files != indexed_files:
        raise SystemExit("release index file inventory differs from the stage")
    entries = {entry["path"]: entry for entry in release_index["files"]}
    for relative in sorted(actual_files):
        path = stage / relative
        entry = entries[relative]
        if entry["bytes"] != path.stat().st_size:
            raise SystemExit(f"release index size differs for {relative}")
        if entry["sha256"] != sha256(path):
            raise SystemExit(f"release index hash differs for {relative}")
        scan_artifact(path)
    if not args.allow_partial:
        actual_labels = {
            path.name for path in artifacts.iterdir() if path.is_dir()
        }
        missing = sorted(required_labels(catalog) - actual_labels)
        if missing:
            raise SystemExit(f"release stage is incomplete: {missing}")
    for required in ("ADMIN.md", "LICENSE-APACHE", "LICENSE-MIT", "PACKAGE.md"):
        if not (stage / required).is_file():
            raise SystemExit(f"release stage is missing {required}")
    print(
        f"HOST_SDK_STAGE_OK version={version} files={len(actual_files)} "
        f"bytes={sum(entry['bytes'] for entry in entries.values())}"
    )


if __name__ == "__main__":
    main()
