#!/usr/bin/env python3
"""Check the app's exact Prns source and Host-contract compatibility record."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
from typing import Any


APPLICATIONS_ROOT = pathlib.Path(__file__).resolve().parents[1]
COMPATIBILITY_PATH = APPLICATIONS_ROOT / "release" / "compatibility.json"
REVISION = re.compile(r"[0-9a-f]{40}\Z")


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


def load_compatibility(path: pathlib.Path = COMPATIBILITY_PATH) -> dict[str, Any]:
    document = object_value(json.loads(path.read_text(encoding="utf-8")), str(path))
    if document.get("schemaVersion") != 1:
        raise fail("schemaVersion must be 1")
    prns = object_value(document.get("prns"), "prns")
    revision = string_value(prns, "revision", "prns")
    if REVISION.fullmatch(revision) is None:
        raise fail("prns.revision must be a full lowercase Git commit ID")
    host = object_value(prns.get("hostContract"), "prns.hostContract")
    identifier = string_value(host, "id", "prns.hostContract")
    fingerprint = string_value(host, "fingerprint", "prns.hostContract")
    if re.fullmatch(rf"{re.escape(identifier)}/[0-9a-f]{{16}}", fingerprint) is None:
        raise fail("prns.hostContract.fingerprint must bind its ID to a 64-bit hex digest")
    javascript = object_value(prns.get("javascriptContract"), "prns.javascriptContract")
    if string_value(javascript, "package", "prns.javascriptContract") != "personal-rns":
        raise fail("prns.javascriptContract.package must be personal-rns")
    if string_value(javascript, "subpath", "prns.javascriptContract") != "./contract":
        raise fail("prns.javascriptContract.subpath must be ./contract")
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
    actual_fingerprint = f"{string_value(host, 'id', 'prns.hostContract')}/{fnv1a64(schema)}"
    if actual_fingerprint != expected_fingerprint:
        raise fail(
            f"Host contract at {revision}:{schema_path} has {actual_fingerprint}, "
            f"expected {expected_fingerprint}"
        )

    javascript = object_value(prns["javascriptContract"], "prns.javascriptContract")
    package_path = string_value(javascript, "sourcePath", "prns.javascriptContract")
    package = object_value(
        json.loads(git(prns_root, "show", f"{revision}:{package_path}/package.json")),
        "Prns JavaScript package",
    )
    expected_package = string_value(javascript, "package", "prns.javascriptContract")
    expected_version = string_value(javascript, "version", "prns.javascriptContract")
    if package.get("name") != expected_package or package.get("version") != expected_version:
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
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1) from error
