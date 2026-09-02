#!/usr/bin/env python3
"""Reject dependencies from base Prns packages into the application island."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path
from typing import Any, Iterable, Iterator

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - repository Python includes tomllib
    try:
        import tomli as tomllib
    except ModuleNotFoundError:
        print(
            "APPLICATION_BOUNDARY_ERROR: Python 3.11+ (or tomli) is required",
            file=sys.stderr,
        )
        raise SystemExit(1)


ROOT = Path(__file__).resolve().parents[2]
PRUNED_DIRECTORIES = {
    ".git",
    ".gradle",
    ".idea",
    ".venv",
    "build",
    "dist",
    "dist-cjs",
    "node_modules",
    "target",
    "vendor",
}
SOURCE_SUFFIXES = {".cjs", ".js", ".jsx", ".mjs", ".rs", ".ts", ".tsx"}
JAVASCRIPT_PATH = re.compile(
    r"(?:\bfrom\s*|\bimport\s*\(\s*|\brequire\s*\(\s*|\bimport\s*)"
    r"[\"'](?P<path>\.{1,2}/[^\"']+)[\"']"
)
RUST_PATH = re.compile(
    r"(?:include|include_bytes|include_str)!\s*\(\s*[\"'](?P<path>[^\"']+)[\"']"
    r"|#\s*\[\s*path\s*=\s*[\"'](?P<attribute>[^\"']+)[\"']\s*\]"
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="repository root to inspect (defaults to this checkout)",
    )
    return parser.parse_args()


def is_within(path: Path, directory: Path) -> bool:
    try:
        path.relative_to(directory)
    except ValueError:
        return False
    return True


def walk_files(root: Path, filename: str | None = None) -> Iterator[Path]:
    if not root.exists():
        return
    for directory, children, files in os.walk(root, followlinks=False):
        children[:] = [child for child in children if child not in PRUNED_DIRECTORIES]
        current = Path(directory)
        for name in files:
            if filename is None or name == filename:
                yield current / name


def load_toml(path: Path) -> dict[str, Any]:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def cargo_manifests(root: Path) -> list[Path]:
    registry = root / "validation" / "manifest.toml"
    if registry.is_file():
        configured = load_toml(registry).get("registry", {}).get("cargo_manifests")
        if isinstance(configured, list) and all(isinstance(item, str) for item in configured):
            return [root / item for item in configured]
    return sorted(walk_files(root, "Cargo.toml"))


def nested_path_values(value: Any, location: str = "") -> Iterator[tuple[str, str]]:
    if isinstance(value, dict):
        for key, child in value.items():
            child_location = f"{location}.{key}" if location else str(key)
            if key == "path" and isinstance(child, str):
                yield child_location, child
            yield from nested_path_values(child, child_location)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from nested_path_values(child, f"{location}[{index}]")


def workspace_entries(workspace: Any, key: str) -> Iterable[str]:
    if not isinstance(workspace, dict):
        return ()
    entries = workspace.get(key, ())
    if isinstance(entries, list):
        return (entry for entry in entries if isinstance(entry, str))
    return ()


def path_expression_reaches_app(expression: str, owner: Path, app_root: Path) -> bool:
    candidate = (owner / expression).resolve(strict=False)
    if is_within(candidate, app_root):
        return True
    if not any(character in expression for character in "*?["):
        return False
    return any(
        is_within(match.resolve(strict=False), app_root)
        for match in owner.glob(expression)
    )


def check_cargo(root: Path, app_root: Path) -> tuple[list[str], set[Path]]:
    violations: list[str] = []
    package_roots: set[Path] = set()
    for manifest_path in cargo_manifests(root):
        resolved_manifest = manifest_path.resolve(strict=False)
        if is_within(resolved_manifest, app_root) or not manifest_path.is_file():
            continue
        manifest = load_toml(manifest_path)
        if isinstance(manifest.get("package"), dict):
            package_roots.add(manifest_path.parent.resolve())
        for location, expression in nested_path_values(manifest):
            if path_expression_reaches_app(expression, manifest_path.parent, app_root):
                violations.append(
                    f"{manifest_path.relative_to(root)}:{location} resolves into applications "
                    f"({expression})"
                )
        workspace = manifest.get("workspace")
        for entry in workspace_entries(workspace, "members"):
            if path_expression_reaches_app(entry, manifest_path.parent, app_root):
                violations.append(
                    f"{manifest_path.relative_to(root)}:workspace.members includes applications "
                    f"({entry})"
                )

    root_manifest_path = root / "Cargo.toml"
    if root_manifest_path.is_file():
        root_manifest = load_toml(root_manifest_path)
        exclusions = tuple(workspace_entries(root_manifest.get("workspace"), "exclude"))
        if not any(path_expression_reaches_app(entry, root, app_root) for entry in exclusions):
            violations.append("Cargo.toml:workspace.exclude must contain applications")
    return violations, package_roots


def dependency_sections(package: dict[str, Any]) -> Iterator[tuple[str, dict[str, Any]]]:
    for section in (
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ):
        dependencies = package.get(section)
        if isinstance(dependencies, dict):
            yield section, dependencies


def local_npm_path(specifier: str) -> str | None:
    for prefix in ("file:", "link:"):
        if specifier.startswith(prefix):
            return specifier.removeprefix(prefix)
    return None


def npm_workspace_entries(package: dict[str, Any]) -> Iterable[str]:
    workspaces = package.get("workspaces", ())
    if isinstance(workspaces, list):
        return (entry for entry in workspaces if isinstance(entry, str))
    if isinstance(workspaces, dict):
        packages = workspaces.get("packages", ())
        if isinstance(packages, list):
            return (entry for entry in packages if isinstance(entry, str))
    return ()


def check_npm(root: Path, app_root: Path) -> tuple[list[str], set[Path]]:
    violations: list[str] = []
    package_roots: set[Path] = set()
    for package_path in sorted(walk_files(root, "package.json")):
        resolved_package = package_path.resolve(strict=False)
        if is_within(resolved_package, app_root):
            continue
        try:
            package = json.loads(package_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            violations.append(f"{package_path.relative_to(root)} could not be read: {error}")
            continue
        if not isinstance(package, dict):
            violations.append(f"{package_path.relative_to(root)} is not a JSON object")
            continue
        package_roots.add(package_path.parent.resolve())
        for section, dependencies in dependency_sections(package):
            for name, specifier in dependencies.items():
                if not isinstance(specifier, str):
                    continue
                local_path = local_npm_path(specifier)
                if local_path is not None and path_expression_reaches_app(
                    local_path, package_path.parent, app_root
                ):
                    violations.append(
                        f"{package_path.relative_to(root)}:{section}.{name} resolves into "
                        f"applications ({specifier})"
                    )
        for entry in npm_workspace_entries(package):
            if path_expression_reaches_app(entry, package_path.parent, app_root):
                violations.append(
                    f"{package_path.relative_to(root)}:workspaces includes applications ({entry})"
                )
    return violations, package_roots


def source_references(path: Path) -> Iterator[str]:
    try:
        source = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return
    if path.suffix in {".cjs", ".js", ".jsx", ".mjs", ".ts", ".tsx"}:
        for match in JAVASCRIPT_PATH.finditer(source):
            yield match.group("path")
    elif path.suffix == ".rs":
        for match in RUST_PATH.finditer(source):
            yield match.group("path") or match.group("attribute")


def check_package_sources(
    root: Path, app_root: Path, package_roots: Iterable[Path]
) -> list[str]:
    violations: list[str] = []
    for package_root in sorted(set(package_roots)):
        for path in walk_files(package_root):
            resolved = path.resolve(strict=False)
            if is_within(resolved, app_root):
                if path.is_symlink():
                    violations.append(
                        f"{path.relative_to(root)} is a base-package symlink into applications"
                    )
                continue
            if path.suffix not in SOURCE_SUFFIXES:
                continue
            for reference in source_references(path):
                candidate = (path.parent / reference.split("?", 1)[0].split("#", 1)[0]).resolve(
                    strict=False
                )
                if is_within(candidate, app_root):
                    violations.append(
                        f"{path.relative_to(root)} imports application source ({reference})"
                    )
    return violations


def main() -> int:
    root = parse_args().root.resolve()
    app_root = (root / "applications").resolve(strict=False)
    cargo_violations, cargo_roots = check_cargo(root, app_root)
    npm_violations, npm_roots = check_npm(root, app_root)
    source_violations = check_package_sources(root, app_root, cargo_roots | npm_roots)
    violations = sorted(set(cargo_violations + npm_violations + source_violations))
    if violations:
        for violation in violations:
            print(f"APPLICATION_BOUNDARY_ERROR: {violation}", file=sys.stderr)
        return 1
    print("APPLICATION_DEPENDENCY_BOUNDARY_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
