#!/usr/bin/env python3
"""Reject production dependencies from reusable packages into applications.

Cargo manifests come from the validation registry. npm manifests and literal
source imports are inspected without resolving/installing dependencies. This is
an architectural guard, not a parser or a proof about computed import paths.
Test/dev dependencies and workspace membership are deliberately outside its scope.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import sys
import tomllib
from typing import Any, Iterator

ROOT = Path(__file__).resolve().parents[2]
PRUNED = {".git", ".gradle", ".venv", ".expo", ".next", "Pods", "node_modules",
          "target", "build", "dist", "dist-cjs", "vendor", "validation-artifacts"}
SOURCE_SUFFIXES = {".rs", ".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"}
TEST_DIRECTORIES = {"tests", "test", "__tests__", "fixtures", "examples", "benches"}
JS_IMPORT = re.compile(r'''(?:\bfrom\s*|\bimport\s*\(\s*|\brequire\s*\(\s*|\bimport\s*)["']([^"']+)["']''')
RUST_IMPORT = re.compile(r'''(?:include(?:_bytes|_str)?!\s*\(\s*|#\s*\[\s*path\s*=\s*)["']([^"']+)["']''')


def inside(path: Path, parent: Path) -> bool:
    return path.resolve().is_relative_to(parent.resolve())


def walk(root: Path, *, production: bool = False) -> Iterator[Path]:
    for directory, children, files in os.walk(root, followlinks=False):
        children[:] = [name for name in children if name not in PRUNED
                       and not (production and name in TEST_DIRECTORIES)]
        for name in files:
            if production and (".test." in name or ".spec." in name):
                continue
            yield Path(directory) / name


def paths(value: Any, location: str = "") -> Iterator[tuple[str, str]]:
    if isinstance(value, dict):
        for key, child in value.items():
            at = f"{location}.{key}" if location else key
            if key == "path" and isinstance(child, str):
                yield at, child
            else:
                yield from paths(child, at)


def production_cargo_paths(manifest: dict[str, Any]) -> Iterator[tuple[str, str]]:
    for name in ("dependencies", "build-dependencies", "patch", "replace"):
        yield from paths(manifest.get(name, {}), name)
    yield from paths(manifest.get("workspace", {}).get("dependencies", {}), "workspace.dependencies")
    for target, tables in manifest.get("target", {}).items():
        for name in ("dependencies", "build-dependencies"):
            yield from paths(tables.get(name, {}), f"target.{target}.{name}")


def source_files(owner: Path, manifest: dict[str, Any], *, cargo: bool) -> set[Path]:
    files: set[Path] = set()
    # Never recursively inspect another package's examples, tools, or build output.
    for directory in ("src",) if cargo else ("src", "lib"):
        files.update(path for path in walk(owner / directory, production=True) if path.suffix in SOURCE_SUFFIXES)
    if cargo:
        entries = [manifest.get("lib", {})] + manifest.get("bin", [])
        files.update(owner / entry["path"] for entry in entries if isinstance(entry.get("path"), str))
        build = manifest.get("package", {}).get("build", "build.rs")
        if isinstance(build, str) and (owner / build).exists():
            files.add(owner / build)
    else:
        files.update(path for path in owner.glob("index.*") if path.suffix in SOURCE_SUFFIXES)
        for name in ("main", "module", "source", "react-native", "types"):
            entry = manifest.get(name)
            if isinstance(entry, str):
                files.add(owner / entry)
    return files


def check(root: Path) -> list[str]:
    app = root / "applications"
    errors: list[str] = []
    sources: set[Path] = set()
    registry = tomllib.loads((root / "validation/manifest.toml").read_text())
    manifests = registry["registry"]["cargo_manifests"]
    if not isinstance(manifests, list) or not all(isinstance(value, str) for value in manifests):
        raise ValueError("registry.cargo_manifests must be a list of paths")

    def location(path: Path) -> str:
        return str(path.relative_to(root))

    def reject_path(path: Path, label: str, reference: str) -> None:
        if inside(path.parent / reference, app):
            errors.append(f"{location(path)}:{label} resolves into applications ({reference})")

    for relative in manifests:
        path = root / relative
        if inside(path, app):
            continue
        manifest = tomllib.loads(path.read_text())
        for label, reference in production_cargo_paths(manifest):
            reject_path(path, label, reference)
        if "package" in manifest:
            sources.update(source_files(path.parent, manifest, cargo=True))

    npm = [(path, json.loads(path.read_text())) for path in walk(root) if path.name == "package.json"]
    app_names = {manifest["name"] for path, manifest in npm if inside(path, app) and isinstance(manifest.get("name"), str)}
    for path, manifest in npm:
        if inside(path, app):
            continue
        for section in ("dependencies", "optionalDependencies", "peerDependencies"):
            for name, specifier in manifest.get(section, {}).items():
                if name in app_names or (isinstance(specifier, str) and any(specifier.startswith(f"npm:{app_name}@") for app_name in app_names)):
                    errors.append(f"{location(path)}:{section}.{name} depends on an application package")
                if not isinstance(specifier, str):
                    continue
                local = False
                for prefix in ("file:", "link:"):
                    if specifier.startswith(prefix):
                        specifier = specifier[len(prefix):]
                        local = True
                        break
                if local or specifier.startswith((".", "/")):
                    reject_path(path, f"{section}.{name}", specifier)
        sources.update(source_files(path.parent, manifest, cargo=False))

    for path in sorted(sources):
        if inside(path, app):
            errors.append(f"{location(path)} is an application source used by a reusable package")
            continue
        if not path.is_file() or path.suffix not in SOURCE_SUFFIXES:
            continue  # Optional/default npm outputs need not have been built.
        source = path.read_text(encoding="utf-8")
        pattern = RUST_IMPORT if path.suffix == ".rs" else JS_IMPORT
        for match in pattern.finditer(source):
            reference = match.group(1).split("?", 1)[0].split("#", 1)[0]
            if path.suffix == ".rs" or reference.startswith((".", "/")):
                reject_path(path, "source import", reference)
            elif path.suffix != ".rs" and any(reference == name or reference.startswith(name + "/") for name in app_names):
                errors.append(f"{location(path)} imports application package {reference}")
    return sorted(set(errors))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args()
    try:
        errors = check(args.root.resolve())
    except (OSError, ValueError, KeyError, TypeError) as error:
        errors = [f"cannot inspect registered package boundary: {error}"]
    for error in errors:
        print(f"APPLICATION_BOUNDARY_ERROR: {error}", file=sys.stderr)
    if errors:
        return 1
    print("APPLICATION_DEPENDENCY_BOUNDARY_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
