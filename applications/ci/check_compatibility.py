#!/usr/bin/env python3
"""Check the app's exact Prns source and Host-contract compatibility record."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tomllib
from typing import Any


APPLICATIONS_ROOT = pathlib.Path(__file__).resolve().parents[1]
COMPATIBILITY_PATH = APPLICATIONS_ROOT / "release" / "compatibility.json"
REVISION = re.compile(r"[0-9a-f]{40}\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
SEMVER = re.compile(r"\d+\.\d+\.\d+\Z")


def fail(message: str) -> ValueError:
    return ValueError(f"compatibility: {message}")


def object_value(value: Any, owner: str) -> dict[str, Any]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise fail(f"{owner} must be a JSON object")
    return value


def string_value(owner: dict[str, Any], key: str, path: str) -> str:
    value = owner.get(key)
    if not isinstance(value, str) or not value:
        raise fail(f"{path}.{key} must be a non-empty string")
    return value


def string_array(owner: dict[str, Any], key: str, path: str) -> list[str]:
    value = owner.get(key)
    if (
        not isinstance(value, list)
        or not value
        or not all(isinstance(entry, str) and entry for entry in value)
    ):
        raise fail(f"{path}.{key} must be a non-empty array of strings")
    if len(set(value)) != len(value):
        raise fail(f"{path}.{key} must not contain duplicates")
    return value


def repository_path(value: str, owner: str) -> str:
    path = pathlib.PurePosixPath(value)
    if path.is_absolute() or not path.parts or ".." in path.parts:
        raise fail(f"{owner} must be a repository-relative path")
    return value


def load_compatibility(path: pathlib.Path = COMPATIBILITY_PATH) -> dict[str, Any]:
    document = object_value(json.loads(path.read_text(encoding="utf-8")), str(path))
    if document.get("schemaVersion") != 2:
        raise fail("schemaVersion must be 2")
    qualification = object_value(document.get("qualification"), "qualification")
    for key in ("node", "npm", "pythonMinimum", "rustMinimum"):
        if SEMVER.fullmatch(string_value(qualification, key, "qualification")) is None:
            raise fail(
                f"qualification.{key} must be an exact major.minor.patch version"
            )
    prns = object_value(document.get("prns"), "prns")
    revision = string_value(prns, "revision", "prns")
    if REVISION.fullmatch(revision) is None:
        raise fail("prns.revision must be a full lowercase Git commit ID")
    host = object_value(prns.get("hostContract"), "prns.hostContract")
    identifier = string_value(host, "id", "prns.hostContract")
    fingerprint = string_value(host, "fingerprint", "prns.hostContract")
    if re.fullmatch(rf"{re.escape(identifier)}/[0-9a-f]{{16}}", fingerprint) is None:
        raise fail(
            "prns.hostContract.fingerprint must bind its ID to a 64-bit hex digest"
        )
    if (
        SHA256.fullmatch(string_value(host, "schemaSha256", "prns.hostContract"))
        is None
    ):
        raise fail("prns.hostContract.schemaSha256 must be a lowercase SHA-256 digest")
    repository_path(
        string_value(host, "schemaPath", "prns.hostContract"),
        "prns.hostContract.schemaPath",
    )

    rust = object_value(prns.get("rustPackages"), "prns.rustPackages")
    direct = string_array(rust, "direct", "prns.rustPackages")
    source = rust.get("source")
    if not isinstance(source, list) or not source:
        raise fail("prns.rustPackages.source must be a non-empty array")
    source_names: set[str] = set()
    source_paths: set[str] = set()
    for index, raw_entry in enumerate(source):
        entry_path = f"prns.rustPackages.source[{index}]"
        entry = object_value(raw_entry, entry_path)
        name = string_value(entry, "name", entry_path)
        package_path = repository_path(
            string_value(entry, "path", entry_path), f"{entry_path}.path"
        )
        version = string_value(entry, "version", entry_path)
        if SEMVER.fullmatch(version) is None:
            raise fail(
                f"{entry_path}.version must be an exact major.minor.patch version"
            )
        if name in source_names or package_path in source_paths:
            raise fail("prns.rustPackages.source names and paths must be unique")
        source_names.add(name)
        source_paths.add(package_path)
    if not set(direct).issubset(source_names):
        raise fail("prns.rustPackages.direct must be a subset of source package names")

    javascript = object_value(prns.get("javascriptContract"), "prns.javascriptContract")
    if string_value(javascript, "package", "prns.javascriptContract") != "personal-rns":
        raise fail("prns.javascriptContract.package must be personal-rns")
    if string_value(javascript, "subpath", "prns.javascriptContract") != "./contract":
        raise fail("prns.javascriptContract.subpath must be ./contract")
    repository_path(
        string_value(javascript, "sourcePath", "prns.javascriptContract"),
        "prns.javascriptContract.sourcePath",
    )
    if (
        SHA256.fullmatch(
            string_value(javascript, "artifactSha256", "prns.javascriptContract")
        )
        is None
    ):
        raise fail(
            "prns.javascriptContract.artifactSha256 must be a lowercase SHA-256 digest"
        )
    for key in ("consumers", "requiredFiles"):
        for index, value in enumerate(
            string_array(javascript, key, "prns.javascriptContract")
        ):
            repository_path(value, f"prns.javascriptContract.{key}[{index}]")
    return document


def git(root: pathlib.Path, *arguments: str) -> bytes:
    return subprocess.run(
        ["git", "-C", str(root), *arguments],
        check=True,
        stdout=subprocess.PIPE,
    ).stdout


def fnv1a64(source: bytes) -> str:
    value = 0xCBF29CE484222325
    for byte in source:
        value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{value:016x}"


def check(prns_root: pathlib.Path, require_head: bool) -> dict[str, Any]:
    compatibility = load_compatibility()
    prns = object_value(compatibility["prns"], "prns")
    revision = string_value(prns, "revision", "prns")
    resolved = git(prns_root, "rev-parse", f"{revision}^{{commit}}").decode().strip()
    if resolved != revision:
        raise fail(f"Prns source resolved {resolved}, expected {revision}")
    if require_head:
        head = git(prns_root, "rev-parse", "HEAD").decode().strip()
        if head != revision:
            raise fail(f"Prns checkout HEAD is {head}, expected {revision}")

    host = object_value(prns["hostContract"], "prns.hostContract")
    schema_path = string_value(host, "schemaPath", "prns.hostContract")
    schema = git(prns_root, "show", f"{revision}:{schema_path}")
    expected_fingerprint = string_value(host, "fingerprint", "prns.hostContract")
    actual_fingerprint = (
        f"{string_value(host, 'id', 'prns.hostContract')}/{fnv1a64(schema)}"
    )
    if actual_fingerprint != expected_fingerprint:
        raise fail(
            f"Host contract at {revision}:{schema_path} has {actual_fingerprint}, "
            f"expected {expected_fingerprint}"
        )
    expected_schema_sha256 = string_value(host, "schemaSha256", "prns.hostContract")
    actual_schema_sha256 = hashlib.sha256(schema).hexdigest()
    if actual_schema_sha256 != expected_schema_sha256:
        raise fail(
            f"Host contract at {revision}:{schema_path} has SHA-256 "
            f"{actual_schema_sha256}, expected {expected_schema_sha256}"
        )

    rust = object_value(prns["rustPackages"], "prns.rustPackages")
    for index, raw_entry in enumerate(rust["source"]):
        entry_path = f"prns.rustPackages.source[{index}]"
        entry = object_value(raw_entry, entry_path)
        package_path = string_value(entry, "path", entry_path)
        manifest = tomllib.loads(
            git(prns_root, "show", f"{revision}:{package_path}/Cargo.toml").decode()
        )
        package = object_value(
            manifest.get("package"), f"{package_path}/Cargo.toml:package"
        )
        expected_name = string_value(entry, "name", entry_path)
        expected_version = string_value(entry, "version", entry_path)
        if (
            package.get("name") != expected_name
            or package.get("version") != expected_version
        ):
            raise fail(
                f"{package_path}/Cargo.toml must identify {expected_name}@{expected_version}"
            )

    javascript = object_value(prns["javascriptContract"], "prns.javascriptContract")
    package_path = string_value(javascript, "sourcePath", "prns.javascriptContract")
    package = object_value(
        json.loads(git(prns_root, "show", f"{revision}:{package_path}/package.json")),
        "Prns JavaScript package",
    )
    expected_package = string_value(javascript, "package", "prns.javascriptContract")
    expected_version = string_value(javascript, "version", "prns.javascriptContract")
    if (
        package.get("name") != expected_package
        or package.get("version") != expected_version
    ):
        raise fail(
            f"{package_path}/package.json must identify {expected_package}@{expected_version}"
        )
    exports = object_value(package.get("exports"), "Prns JavaScript exports")
    subpath = string_value(javascript, "subpath", "prns.javascriptContract")
    if subpath not in exports:
        raise fail(f"{expected_package}@{expected_version} does not export {subpath}")
    return compatibility


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--prns-root",
        type=pathlib.Path,
        default=pathlib.Path(
            os.environ.get("PRNS_COMPATIBILITY_PRNS_ROOT", APPLICATIONS_ROOT.parent)
        ),
    )
    parser.add_argument("--require-head", action="store_true")
    return parser.parse_args()


def main() -> int:
    selected = arguments()
    compatibility = check(selected.prns_root.resolve(), selected.require_head)
    prns = object_value(compatibility["prns"], "prns")
    print(
        "APPLICATION_COMPATIBILITY_OK "
        f"revision={string_value(prns, 'revision', 'prns')} "
        f"host={string_value(object_value(prns['hostContract'], 'prns.hostContract'), 'fingerprint', 'prns.hostContract')}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (
        OSError,
        ValueError,
        subprocess.CalledProcessError,
        tomllib.TOMLDecodeError,
    ) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1) from error
