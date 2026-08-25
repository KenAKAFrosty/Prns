from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

GENERIC_PAGES = ("help.html", "settings.html")
CURRENT_CRATE = re.compile(rb'data-current-crate="[^"]+"')
CRATE_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
PACKAGE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]*$")


def workspace_package_names(metadata: object) -> list[str]:
    if not isinstance(metadata, dict):
        raise ValueError("Cargo metadata must be an object")
    packages = metadata.get("packages")
    members = metadata.get("workspace_members")
    if not isinstance(packages, list) or not isinstance(members, list) or not members:
        raise ValueError("Cargo metadata has no workspace packages")

    packages_by_id: dict[str, str] = {}
    for package in packages:
        if not isinstance(package, dict):
            raise ValueError("Cargo metadata package is malformed")
        package_id = package.get("id")
        name = package.get("name")
        if (
            not isinstance(package_id, str)
            or not isinstance(name, str)
            or PACKAGE_NAME.fullmatch(name) is None
        ):
            raise ValueError("Cargo metadata package identity is malformed")
        if package_id in packages_by_id:
            raise ValueError(f"Cargo metadata repeats package ID: {package_id}")
        packages_by_id[package_id] = name

    names: list[str] = []
    for package_id in members:
        if not isinstance(package_id, str) or package_id not in packages_by_id:
            raise ValueError(
                f"Cargo metadata workspace member is unavailable: {package_id}"
            )
        names.append(packages_by_id[package_id])
    if len(names) != len(set(names)):
        raise ValueError("Cargo metadata repeats a workspace package name")
    return sorted(names)


def normalize_generic_pages(output: Path, current_crate: str) -> None:
    if not output.is_dir():
        raise ValueError("Rustdoc output directory is unavailable")
    if CRATE_NAME.fullmatch(current_crate) is None:
        raise ValueError("Rustdoc current crate is invalid")
    if not (output / current_crate / "index.html").is_file():
        raise ValueError("Rustdoc current crate is absent from the output")

    replacement = f'data-current-crate="{current_crate}"'.encode()
    normalized: dict[Path, bytes] = {}
    for relative in GENERIC_PAGES:
        path = output / relative
        if not path.is_file():
            raise ValueError(f"Rustdoc generic page is unavailable: {relative}")
        value, replacements = CURRENT_CRATE.subn(replacement, path.read_bytes())
        if replacements != 1:
            raise ValueError(
                f"Rustdoc generic page has {replacements} current-crate fields: {relative}"
            )
        normalized[path] = value

    for path, value in normalized.items():
        path.write_bytes(value)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path, nargs="?")
    parser.add_argument("--current-crate")
    parser.add_argument("--list-workspace-packages", action="store_true")
    arguments = parser.parse_args()
    try:
        if arguments.list_workspace_packages:
            if arguments.output is not None or arguments.current_crate is not None:
                raise ValueError(
                    "workspace package listing does not accept Rustdoc output arguments"
                )
            names = workspace_package_names(json.load(sys.stdin))
            sys.stdout.write("".join(f"{name}\n" for name in names))
            return 0
        if arguments.output is None or arguments.current_crate is None:
            raise ValueError("Rustdoc output and current crate are required")
        normalize_generic_pages(arguments.output, arguments.current_crate)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Rustdoc normalization failed: {error}", file=sys.stderr)
        return 1
    print(f"normalized Rustdoc generic pages to {arguments.current_crate}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
