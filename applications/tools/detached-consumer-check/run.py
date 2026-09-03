#!/usr/bin/env python3
"""Export and qualify the application workspace against one exact Prns revision."""

from __future__ import annotations

import argparse
import base64
import copy
import hashlib
import json
import os
import pathlib
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.parse
from dataclasses import dataclass
from typing import Any, Iterable


APPLICATIONS_ROOT = pathlib.Path(__file__).resolve().parents[2]
REPOSITORY_ROOT = APPLICATIONS_ROOT.parent
COMPATIBILITY_PATH = APPLICATIONS_ROOT / "release" / "compatibility.json"
REVISION = re.compile(r"[0-9a-f]{40}\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
SEMVER = re.compile(r"\d+\.\d+\.\d+\Z")
INLINE_DEPENDENCY = re.compile(
    r"^(?P<prefix>\s*(?P<name>[A-Za-z0-9_-]+)\s*=\s*\{)(?P<body>.*)(?P<suffix>\}\s*(?:#.*)?)$"
)
PATH_ATTRIBUTE = re.compile(r'(?<![A-Za-z0-9_-])path\s*=\s*"(?P<path>[^"]+)"')
PACKAGE_ATTRIBUTE = re.compile(r'(?<![A-Za-z0-9_-])package\s*=\s*"(?P<package>[^"]+)"')
DEPENDENCY_SECTIONS = (
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
)


class QualificationFailure(RuntimeError):
    """A detached qualification invariant was not met."""


@dataclass(frozen=True)
class CargoRewrite:
    manifest: pathlib.Path
    line_index: int
    original: str
    dependency: str
    package: str


@dataclass(frozen=True)
class NpmRewrite:
    manifest: pathlib.Path
    section: str
    dependency: str
    original: str


@dataclass(frozen=True)
class RustPackage:
    name: str
    path: pathlib.PurePosixPath
    version: str


def fail(message: str) -> QualificationFailure:
    return QualificationFailure(message)


def run(
    arguments: Iterable[str | os.PathLike[str]],
    *,
    cwd: pathlib.Path,
    environment: dict[str, str] | None = None,
    capture: bool = False,
) -> subprocess.CompletedProcess[str]:
    command = [os.fspath(argument) for argument in arguments]
    print(f"+ {shlex.join(command)}", flush=True)
    return subprocess.run(
        command,
        cwd=cwd,
        env=environment,
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else None,
    )


def git_output(root: pathlib.Path, *arguments: str) -> str:
    return run(("git", "-C", root, *arguments), cwd=root, capture=True).stdout.strip()


def parsed_version(value: str, owner: str) -> tuple[int, int, int]:
    match = re.search(r"(?<!\d)(\d+)\.(\d+)\.(\d+)(?!\d)", value)
    if match is None:
        raise fail(f"{owner} emitted an unrecognized version: {value.strip()}")
    return tuple(int(part) for part in match.groups())  # type: ignore[return-value]


def require_qualification_toolchains(
    compatibility: dict[str, Any], repository_root: pathlib.Path
) -> None:
    qualification = object_value(
        compatibility["qualification"], "compatibility.qualification"
    )
    expected_node = string_value(qualification, "node", "compatibility.qualification")
    expected_npm = string_value(qualification, "npm", "compatibility.qualification")
    actual_node = run(
        ("node", "--version"), cwd=repository_root, capture=True
    ).stdout.strip()
    actual_npm = run(
        ("npm", "--version"), cwd=repository_root, capture=True
    ).stdout.strip()
    if actual_node.removeprefix("v") != expected_node:
        raise fail(f"Node {expected_node} is required, received {actual_node}")
    if actual_npm != expected_npm:
        raise fail(f"npm {expected_npm} is required, received {actual_npm}")

    expected_python = parsed_version(
        string_value(qualification, "pythonMinimum", "compatibility.qualification"),
        "compatibility Python minimum",
    )
    actual_python = sys.version_info[:3]
    if actual_python < expected_python:
        raise fail(
            f"Python {'.'.join(map(str, expected_python))} or newer is required, "
            f"received {'.'.join(map(str, actual_python))}"
        )
    expected_rust = parsed_version(
        string_value(qualification, "rustMinimum", "compatibility.qualification"),
        "compatibility Rust minimum",
    )
    rustc = run(
        ("rustc", "--version"), cwd=repository_root, capture=True
    ).stdout.strip()
    if parsed_version(rustc, "rustc") < expected_rust:
        raise fail(
            f"Rust {'.'.join(map(str, expected_rust))} or newer is required, received {rustc}"
        )


def controlled_environment(
    work: pathlib.Path, applications_root: pathlib.Path
) -> dict[str, str]:
    environment = os.environ.copy()
    forbidden_prefixes = ("CARGO_", "NPM_CONFIG_", "PRNS_", "RUST", "UV_")
    forbidden_names = {"LXMF_VENV", "NODE_OPTIONS", "PYTHONHOME", "PYTHONPATH"}
    for key in list(environment):
        if key.upper().startswith(forbidden_prefixes) or key.upper() in forbidden_names:
            environment.pop(key)
    cargo_home = work / "cargo-home"
    cargo_home.mkdir()
    npm_cache = work / "npm-cache"
    npm_cache.mkdir()
    uv_cache = work / "uv-cache"
    uv_cache.mkdir()
    npm_user_config = work / "npm-user.npmrc"
    npm_user_config.write_text("", encoding="utf-8")
    npm_global_config = work / "npm-global.npmrc"
    npm_global_config.write_text("", encoding="utf-8")
    environment.update(
        {
            "CARGO_HOME": os.fspath(cargo_home),
            "CARGO_NET_GIT_FETCH_WITH_CLI": "true",
            "CARGO_TARGET_DIR": os.fspath(applications_root / "target"),
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_SYSTEM": os.devnull,
            "NPM_CONFIG_CACHE": os.fspath(npm_cache),
            "NPM_CONFIG_GLOBALCONFIG": os.fspath(npm_global_config),
            "NPM_CONFIG_USERCONFIG": os.fspath(npm_user_config),
            "RUST_MIN_STACK": str(16 * 1024 * 1024),
            "UV_CACHE_DIR": os.fspath(uv_cache),
        }
    )
    return environment


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


def repository_path(value: str, owner: str) -> pathlib.PurePosixPath:
    path = pathlib.PurePosixPath(value)
    if path.is_absolute() or not path.parts or ".." in path.parts:
        raise fail(f"{owner} must be a repository-relative path")
    return path


def load_compatibility() -> dict[str, Any]:
    document = object_value(
        json.loads(COMPATIBILITY_PATH.read_text(encoding="utf-8")),
        str(COMPATIBILITY_PATH),
    )
    if document.get("schemaVersion") != 2:
        raise fail("release/compatibility.json schemaVersion must be 2")
    qualification = object_value(
        document.get("qualification"), "compatibility.qualification"
    )
    for key in ("node", "npm", "pythonMinimum", "rustMinimum"):
        if (
            SEMVER.fullmatch(
                string_value(qualification, key, "compatibility.qualification")
            )
            is None
        ):
            raise fail(f"compatibility.qualification.{key} must be major.minor.patch")
    prns = object_value(document.get("prns"), "compatibility.prns")
    revision = string_value(prns, "revision", "compatibility.prns")
    if REVISION.fullmatch(revision) is None:
        raise fail("compatibility.prns.revision must be a full lowercase Git commit ID")
    rust_packages(document)
    javascript = object_value(
        prns.get("javascriptContract"), "compatibility.prns.javascriptContract"
    )
    for key in ("consumers", "requiredFiles"):
        for index, value in enumerate(
            string_array(javascript, key, "compatibility.prns.javascriptContract")
        ):
            repository_path(
                value, f"compatibility.prns.javascriptContract.{key}[{index}]"
            )
    if (
        SHA256.fullmatch(
            string_value(
                javascript, "artifactSha256", "compatibility.prns.javascriptContract"
            )
        )
        is None
    ):
        raise fail(
            "compatibility JavaScript artifact SHA-256 must be lowercase hexadecimal"
        )
    return document


def rust_packages(
    compatibility: dict[str, Any],
) -> tuple[frozenset[str], dict[str, RustPackage]]:
    prns = object_value(compatibility.get("prns"), "compatibility.prns")
    rust = object_value(prns.get("rustPackages"), "compatibility.prns.rustPackages")
    direct = frozenset(string_array(rust, "direct", "compatibility.prns.rustPackages"))
    raw_source = rust.get("source")
    if not isinstance(raw_source, list) or not raw_source:
        raise fail("compatibility.prns.rustPackages.source must be a non-empty array")
    source: dict[str, RustPackage] = {}
    seen_paths: set[pathlib.PurePosixPath] = set()
    for index, raw_entry in enumerate(raw_source):
        owner = f"compatibility.prns.rustPackages.source[{index}]"
        entry = object_value(raw_entry, owner)
        name = string_value(entry, "name", owner)
        path = repository_path(string_value(entry, "path", owner), f"{owner}.path")
        version = string_value(entry, "version", owner)
        if SEMVER.fullmatch(version) is None:
            raise fail(f"{owner}.version must be major.minor.patch")
        if name in source or path in seen_paths:
            raise fail("compatibility Rust package names and paths must be unique")
        source[name] = RustPackage(name=name, path=path, version=version)
        seen_paths.add(path)
    if not direct.issubset(source):
        raise fail(
            "compatibility direct Rust packages must be declared source packages"
        )
    return direct, source


def relative_to(path: pathlib.Path, root: pathlib.Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except ValueError:
        return False


def tracked_base_fingerprint(repository_root: pathlib.Path) -> str:
    paths = subprocess.run(
        ("git", "-C", str(repository_root), "ls-files", "-z"),
        check=True,
        stdout=subprocess.PIPE,
    ).stdout.split(b"\0")
    digest = hashlib.sha256()
    for encoded in paths:
        if not encoded:
            continue
        relative = pathlib.PurePosixPath(os.fsdecode(encoded))
        if relative.parts[0] == "applications":
            continue
        path = repository_root / pathlib.Path(relative)
        digest.update(encoded)
        digest.update(b"\0")
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            digest.update(b"missing\0")
            continue
        digest.update(f"{stat.S_IFMT(metadata.st_mode):o}\0".encode())
        if path.is_symlink():
            digest.update(os.fsencode(os.readlink(path)))
        else:
            digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def base_status(repository_root: pathlib.Path) -> bytes:
    return subprocess.run(
        (
            "git",
            "-C",
            str(repository_root),
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
            ":(exclude)applications/**",
        ),
        check=True,
        stdout=subprocess.PIPE,
    ).stdout


def require_clean_application_source(repository_root: pathlib.Path) -> None:
    status_output = subprocess.run(
        (
            "git",
            "-C",
            str(repository_root),
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--",
            "applications",
        ),
        check=True,
        stdout=subprocess.PIPE,
    ).stdout
    if status_output:
        raise fail(
            "applications/ must be committed before its tracked export is qualified"
        )


def reject_tracked_symlinks(repository_root: pathlib.Path) -> None:
    listing = git_output(repository_root, "ls-files", "--stage", "--", "applications")
    symlinks = []
    for line in listing.splitlines():
        metadata, _, path = line.partition("\t")
        mode = metadata.split(maxsplit=1)[0]
        if mode == "120000":
            symlinks.append(path)
    if symlinks:
        raise fail(f"tracked application symlinks are forbidden: {', '.join(symlinks)}")


def export_applications(
    repository_root: pathlib.Path, destination: pathlib.Path
) -> pathlib.Path:
    archive = destination / "applications.tar"
    run(
        (
            "git",
            "-C",
            repository_root,
            "archive",
            "--format=tar",
            "-o",
            archive,
            "HEAD",
            "applications",
        ),
        cwd=repository_root,
    )
    export_root = destination / "export"
    export_root.mkdir()
    with tarfile.open(archive, "r:") as source:
        members = source.getmembers()
        for member in members:
            path = pathlib.PurePosixPath(member.name)
            if (
                not path.parts
                or path.parts[0] != "applications"
                or path.is_absolute()
                or ".." in path.parts
            ):
                raise fail(f"Git archive contains an unsafe path: {member.name}")
            if member.issym() or member.islnk() or member.isdev():
                raise fail(f"Git archive contains a non-file entry: {member.name}")
        for member in members:
            destination_path = export_root / pathlib.Path(member.name)
            if member.isdir():
                destination_path.mkdir(parents=True, exist_ok=True)
                continue
            if not member.isfile():
                raise fail(f"Git archive contains an unsupported entry: {member.name}")
            destination_path.parent.mkdir(parents=True, exist_ok=True)
            archived_file = source.extractfile(member)
            if archived_file is None:
                raise fail(f"Git archive file is unreadable: {member.name}")
            with archived_file, destination_path.open("wb") as output:
                shutil.copyfileobj(archived_file, output)
            destination_path.chmod(member.mode & 0o777)
    applications_root = export_root / "applications"
    for path in applications_root.rglob("*"):
        if path.is_symlink():
            raise fail(f"exported application symlink is forbidden: {path}")
    return applications_root


def cargo_package_identity(manifest: pathlib.Path) -> tuple[str, str]:
    document = tomllib.loads(manifest.read_text(encoding="utf-8"))
    package = document.get("package")
    if (
        not isinstance(package, dict)
        or not isinstance(package.get("name"), str)
        or not isinstance(package.get("version"), str)
    ):
        raise fail(f"{manifest} does not declare package.name and package.version")
    return package["name"], package["version"]


def manifests_below(root: pathlib.Path, filename: str) -> Iterable[pathlib.Path]:
    excluded = {"node_modules", "target", "vendor", "__pycache__"}
    for path in sorted(root.rglob(filename)):
        relative = path.relative_to(root)
        if not excluded.intersection(relative.parts):
            yield path


def reject_resolver_configuration(applications_root: pathlib.Path) -> None:
    forbidden = []
    for path in applications_root.rglob("*"):
        if not path.is_file():
            continue
        if path.name == ".npmrc" or (
            path.parent.name == ".cargo" and path.name in {"config", "config.toml"}
        ):
            forbidden.append(path.relative_to(applications_root))
    if forbidden:
        raise fail(
            "detached qualification forbids local resolver configuration: "
            + ", ".join(map(str, sorted(forbidden)))
        )


def cargo_rewrite_plan(
    applications_root: pathlib.Path,
    repository_root: pathlib.Path,
    compatibility: dict[str, Any],
) -> list[CargoRewrite]:
    direct_packages, source_packages = rust_packages(compatibility)
    result = []
    for manifest in manifests_below(applications_root, "Cargo.toml"):
        lines = manifest.read_text(encoding="utf-8").splitlines(keepends=True)
        for line_index, line in enumerate(lines):
            declaration = INLINE_DEPENDENCY.match(line.rstrip("\n"))
            if declaration is None:
                continue
            path_match = PATH_ATTRIBUTE.search(declaration.group("body"))
            if path_match is None:
                continue
            dependency_path = (manifest.parent / path_match.group("path")).resolve()
            if relative_to(dependency_path, applications_root):
                continue
            if not relative_to(dependency_path, repository_root):
                raise fail(
                    f"Cargo dependency escapes the Prns repository: {manifest}:{line_index + 1}"
                )
            dependency_manifest = dependency_path / "Cargo.toml"
            if not dependency_manifest.is_file():
                raise fail(f"Cargo dependency has no manifest: {dependency_path}")
            package_match = PACKAGE_ATTRIBUTE.search(declaration.group("body"))
            expected_package = (
                package_match.group("package")
                if package_match is not None
                else declaration.group("name")
            )
            actual_package, actual_version = cargo_package_identity(dependency_manifest)
            expected_source = source_packages.get(expected_package)
            expected_dependency_path = (
                repository_root / pathlib.Path(*expected_source.path.parts)
                if expected_source is not None
                else None
            )
            if (
                actual_package != expected_package
                or expected_package not in direct_packages
                or expected_source is None
                or dependency_path != expected_dependency_path.resolve()
                or actual_version != expected_source.version
            ):
                raise fail(
                    f"unreviewed external Cargo dependency {expected_package} at "
                    f"{manifest}:{line_index + 1}"
                )
            result.append(
                CargoRewrite(
                    manifest=manifest.relative_to(applications_root),
                    line_index=line_index,
                    original=path_match.group("path"),
                    dependency=declaration.group("name"),
                    package=actual_package,
                )
            )
    planned_paths = sorted((rewrite.manifest, rewrite.original) for rewrite in result)
    discovered_paths: list[tuple[pathlib.Path, str]] = []
    for manifest in manifests_below(applications_root, "Cargo.toml"):
        document = tomllib.loads(manifest.read_text(encoding="utf-8"))
        for path_value in walk_path_values(document):
            resolved = (manifest.parent / path_value).resolve()
            if not relative_to(resolved, applications_root):
                discovered_paths.append(
                    (manifest.relative_to(applications_root), path_value)
                )
    if sorted(discovered_paths) != planned_paths:
        raise fail(
            "external Cargo path scanner found an unsupported or unreviewed declaration"
        )
    if {rewrite.package for rewrite in result} != direct_packages:
        raise fail(
            "external Cargo dependency inventory does not match the application contract"
        )
    return result


def rewrite_cargo_dependencies(
    applications_root: pathlib.Path,
    rewrites: list[CargoRewrite],
    git_url: str,
    revision: str,
) -> None:
    by_manifest: dict[pathlib.Path, list[CargoRewrite]] = {}
    for rewrite in rewrites:
        by_manifest.setdefault(rewrite.manifest, []).append(rewrite)
    replacement = f"git = {json.dumps(git_url)}, rev = {json.dumps(revision)}"
    for relative_manifest, manifest_rewrites in by_manifest.items():
        manifest = applications_root / relative_manifest
        lines = manifest.read_text(encoding="utf-8").splitlines(keepends=True)
        for rewrite in manifest_rewrites:
            path_expression = re.compile(
                rf"(?<![A-Za-z0-9_-])path\s*=\s*{re.escape(json.dumps(rewrite.original))}"
            )
            updated, count = path_expression.subn(
                replacement, lines[rewrite.line_index], count=1
            )
            if count != 1:
                raise fail(
                    f"could not rewrite Cargo dependency at {relative_manifest}:{rewrite.line_index + 1}"
                )
            lines[rewrite.line_index] = updated
        manifest.write_text("".join(lines), encoding="utf-8")


def walk_path_values(value: Any) -> Iterable[str]:
    if isinstance(value, dict):
        for key, child in value.items():
            if key == "path" and isinstance(child, str):
                yield child
            yield from walk_path_values(child)
    elif isinstance(value, list):
        for child in value:
            yield from walk_path_values(child)


def reject_external_cargo_paths(applications_root: pathlib.Path) -> None:
    for manifest in manifests_below(applications_root, "Cargo.toml"):
        document = tomllib.loads(manifest.read_text(encoding="utf-8"))
        for path_value in walk_path_values(document):
            resolved = (manifest.parent / path_value).resolve()
            if not relative_to(resolved, applications_root):
                raise fail(
                    f"Cargo path escapes detached applications/: {manifest}: {path_value}"
                )


def npm_local_path(selection: str) -> str | None:
    for prefix in ("file:", "link:"):
        if selection.startswith(prefix):
            return selection.removeprefix(prefix)
    if (
        selection.startswith(("./", "../", "/", "~"))
        or re.match(r"^[A-Za-z]:[\\/]", selection) is not None
    ):
        return selection
    return None


def npm_rewrite_plan(
    applications_root: pathlib.Path,
    repository_root: pathlib.Path,
    compatibility: dict[str, Any],
) -> list[NpmRewrite]:
    prns = object_value(compatibility["prns"], "compatibility.prns")
    javascript = object_value(
        prns["javascriptContract"], "compatibility.prns.javascriptContract"
    )
    expected_consumers = {
        pathlib.Path(*repository_path(value, "compatibility JavaScript consumer").parts)
        for value in string_array(
            javascript, "consumers", "compatibility.prns.javascriptContract"
        )
    }
    expected_source = (
        repository_root
        / pathlib.Path(
            *repository_path(
                string_value(
                    javascript, "sourcePath", "compatibility.prns.javascriptContract"
                ),
                "compatibility JavaScript source",
            ).parts
        )
    ).resolve()
    expected_package_name = string_value(
        javascript, "package", "compatibility.prns.javascriptContract"
    )
    result = []
    for manifest in manifests_below(applications_root, "package.json"):
        package = object_value(
            json.loads(manifest.read_text(encoding="utf-8")), str(manifest)
        )
        for section in DEPENDENCY_SECTIONS:
            dependencies = package.get(section)
            if dependencies is None:
                continue
            dependencies = object_value(dependencies, f"{manifest}:{section}")
            for dependency, selection in dependencies.items():
                if not isinstance(selection, str):
                    continue
                local_path = npm_local_path(selection)
                if local_path is None:
                    continue
                dependency_path = (manifest.parent / local_path).resolve()
                if relative_to(dependency_path, applications_root):
                    continue
                if not relative_to(dependency_path, repository_root):
                    raise fail(
                        f"npm dependency escapes the Prns repository: {manifest}:{dependency}"
                    )
                dependency_manifest = dependency_path / "package.json"
                if not dependency_manifest.is_file():
                    raise fail(f"npm dependency has no package.json: {dependency_path}")
                dependency_package = object_value(
                    json.loads(dependency_manifest.read_text(encoding="utf-8")),
                    str(dependency_manifest),
                )
                relative_manifest = manifest.relative_to(applications_root)
                if (
                    dependency != expected_package_name
                    or dependency_package.get("name") != dependency
                    or dependency_path != expected_source
                    or relative_manifest not in expected_consumers
                ):
                    raise fail(
                        f"unreviewed external npm dependency {dependency} in {manifest}"
                    )
                result.append(
                    NpmRewrite(
                        manifest=relative_manifest,
                        section=section,
                        dependency=dependency,
                        original=selection,
                    )
                )
    actual_consumers = {rewrite.manifest for rewrite in result}
    if actual_consumers != expected_consumers or len(result) != len(expected_consumers):
        raise fail(
            "external npm dependency inventory does not match the compatibility consumers"
        )
    return result


def rewrite_npm_dependencies(
    applications_root: pathlib.Path,
    rewrites: list[NpmRewrite],
    artifact: pathlib.Path,
) -> str:
    selections = set()
    for rewrite in rewrites:
        manifest = applications_root / rewrite.manifest
        package = object_value(
            json.loads(manifest.read_text(encoding="utf-8")), str(manifest)
        )
        dependencies = object_value(
            package[rewrite.section], f"{manifest}:{rewrite.section}"
        )
        if dependencies.get(rewrite.dependency) != rewrite.original:
            raise fail(
                f"npm dependency changed before rewrite: {manifest}:{rewrite.dependency}"
            )
        relative_artifact = os.path.relpath(artifact, manifest.parent)
        selection = f"file:{pathlib.PurePath(relative_artifact).as_posix()}"
        dependencies[rewrite.dependency] = selection
        manifest.write_text(f"{json.dumps(package, indent=2)}\n", encoding="utf-8")
        selections.add(selection)
    if len(selections) != 1:
        raise fail(
            "detached npm workspaces must use one shared personal-rns artifact selection"
        )
    return selections.pop()


def walk_json_strings(value: Any) -> Iterable[str]:
    if isinstance(value, dict):
        for child in value.values():
            yield from walk_json_strings(child)
    elif isinstance(value, list):
        for child in value:
            yield from walk_json_strings(child)
    elif isinstance(value, str):
        yield value


def reject_external_npm_paths(applications_root: pathlib.Path) -> None:
    for manifest in manifests_below(applications_root, "package.json"):
        package = object_value(
            json.loads(manifest.read_text(encoding="utf-8")), str(manifest)
        )
        for selection in walk_json_strings(package):
            local_path = npm_local_path(selection)
            if local_path is None:
                continue
            resolved = (manifest.parent / local_path).resolve()
            if not relative_to(resolved, applications_root):
                raise fail(
                    f"npm local path escapes detached applications/: {manifest}: {selection}"
                )


def reject_external_npm_lock_paths(applications_root: pathlib.Path) -> None:
    lock = object_value(
        json.loads(
            (applications_root / "package-lock.json").read_text(encoding="utf-8")
        ),
        "detached package-lock.json",
    )
    packages = object_value(lock.get("packages"), "detached package-lock packages")
    for key, raw_entry in packages.items():
        if key.startswith("../") or pathlib.PurePosixPath(key).is_absolute():
            raise fail(f"npm lock package key escapes detached applications/: {key}")
        entry = object_value(raw_entry, f"detached package-lock packages.{key}")
        resolved = entry.get("resolved")
        if not isinstance(resolved, str):
            continue
        local_path = npm_local_path(resolved)
        if local_path is None:
            continue
        if not relative_to(
            (applications_root / local_path).resolve(), applications_root
        ):
            raise fail(
                f"npm lock resolution escapes detached applications/: {key}: {resolved}"
            )


def checkout_prns(
    git_url: str,
    revision: str,
    destination: pathlib.Path,
    environment: dict[str, str],
) -> None:
    destination.mkdir()
    run(("git", "init", "--quiet"), cwd=destination, environment=environment)
    run(
        ("git", "remote", "add", "origin", git_url),
        cwd=destination,
        environment=environment,
    )
    run(
        (
            "git",
            "-c",
            "protocol.file.allow=always",
            "fetch",
            "--no-tags",
            "--depth=1",
            "origin",
            revision,
        ),
        cwd=destination,
        environment=environment,
    )
    run(
        ("git", "checkout", "--quiet", "--detach", "FETCH_HEAD"),
        cwd=destination,
        environment=environment,
    )
    resolved = run(
        ("git", "rev-parse", "HEAD"),
        cwd=destination,
        environment=environment,
        capture=True,
    ).stdout.strip()
    if resolved != revision:
        raise fail(f"Prns checkout resolved {resolved}, expected {revision}")


def build_javascript_artifact(
    prns_root: pathlib.Path,
    applications_root: pathlib.Path,
    compatibility: dict[str, Any],
    environment: dict[str, str],
) -> tuple[pathlib.Path, str, str]:
    prns = object_value(compatibility["prns"], "compatibility.prns")
    javascript = object_value(
        prns["javascriptContract"], "compatibility.prns.javascriptContract"
    )
    source = prns_root / string_value(
        javascript, "sourcePath", "compatibility.prns.javascriptContract"
    )
    run(
        ("npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"),
        cwd=source,
        environment=environment,
    )
    run(("npm", "run", "build:code"), cwd=source, environment=environment)
    vendor = applications_root / "vendor"
    vendor.mkdir()
    packed = run(
        ("npm", "pack", "--json", "--pack-destination", vendor),
        cwd=source,
        environment=environment,
        capture=True,
    )
    pack_result = json.loads(packed.stdout)
    if not isinstance(pack_result, list) or len(pack_result) != 1:
        raise fail("npm pack did not report exactly one personal-rns artifact")
    entry = object_value(pack_result[0], "npm pack result")
    filename = string_value(entry, "filename", "npm pack result")
    artifact = vendor / filename
    if not artifact.is_file():
        raise fail(f"npm pack did not create {artifact}")

    required = set(
        string_array(
            javascript, "requiredFiles", "compatibility.prns.javascriptContract"
        )
    )
    with tarfile.open(artifact, "r:gz") as package:
        names = set()
        for member in package.getmembers():
            path = pathlib.PurePosixPath(member.name)
            if (
                path.is_absolute()
                or ".." in path.parts
                or member.issym()
                or member.islnk()
            ):
                raise fail(
                    f"personal-rns artifact contains an unsafe path: {member.name}"
                )
            names.add(member.name)
        missing = required - names
        if missing:
            raise fail(
                f"personal-rns artifact is missing: {', '.join(sorted(missing))}"
            )
        package_file = package.extractfile("package/package.json")
        if package_file is None:
            raise fail("personal-rns artifact package.json is unreadable")
        metadata = object_value(
            json.load(package_file), "packed personal-rns package.json"
        )
    expected_name = string_value(
        javascript, "package", "compatibility.prns.javascriptContract"
    )
    expected_version = string_value(
        javascript, "version", "compatibility.prns.javascriptContract"
    )
    if (
        metadata.get("name") != expected_name
        or metadata.get("version") != expected_version
    ):
        raise fail(f"packed artifact must identify {expected_name}@{expected_version}")
    exports = object_value(metadata.get("exports"), "packed personal-rns exports")
    subpath = string_value(
        javascript, "subpath", "compatibility.prns.javascriptContract"
    )
    if subpath not in exports:
        raise fail(f"packed personal-rns artifact does not export {subpath}")
    artifact_bytes = artifact.read_bytes()
    digest = hashlib.sha256(artifact_bytes).hexdigest()
    expected_digest = string_value(
        javascript, "artifactSha256", "compatibility.prns.javascriptContract"
    )
    if digest != expected_digest:
        raise fail(
            f"packed personal-rns artifact has SHA-256 {digest}, expected {expected_digest}"
        )
    integrity = (
        "sha512-" + base64.b64encode(hashlib.sha512(artifact_bytes).digest()).decode()
    )
    return artifact, digest, integrity


def normalized_repository_url(value: str) -> str:
    parsed = urllib.parse.urlsplit(value)
    path = urllib.parse.unquote(parsed.path).rstrip("/")
    if path.endswith(".git"):
        path = path[:-4]
    return urllib.parse.urlunsplit(
        (parsed.scheme.lower(), parsed.netloc.lower(), path, "", "")
    )


def exact_git_source(
    source: object, git_url: str, revision: str, *, resolved: bool
) -> bool:
    if not isinstance(source, str) or not source.startswith("git+"):
        return False
    source_without_prefix = source.removeprefix("git+")
    repository_and_query, separator, resolved_revision = (
        source_without_prefix.rpartition("#")
    )
    if resolved:
        if separator != "#" or resolved_revision != revision:
            return False
    elif separator:
        return False
    parsed = urllib.parse.urlsplit(repository_and_query)
    query = urllib.parse.parse_qs(parsed.query, strict_parsing=True)
    if query != {"rev": [revision]}:
        return False
    repository = urllib.parse.urlunsplit(
        (parsed.scheme, parsed.netloc, parsed.path, "", "")
    )
    return normalized_repository_url(repository) == normalized_repository_url(git_url)


def lock_package_map(
    document: dict[str, Any], owner: str
) -> dict[tuple[str, str], dict[str, Any]]:
    raw_packages = document.get("package")
    if not isinstance(raw_packages, list):
        raise fail(f"{owner}.package must be an array")
    result: dict[tuple[str, str], dict[str, Any]] = {}
    for index, raw_package in enumerate(raw_packages):
        package = object_value(raw_package, f"{owner}.package[{index}]")
        name = string_value(package, "name", f"{owner}.package[{index}]")
        version = string_value(package, "version", f"{owner}.package[{index}]")
        key = (name, version)
        if key in result:
            raise fail(f"{owner} contains duplicate package {name}@{version}")
        result[key] = package
    return result


def validate_cargo_lock_refresh(
    before_bytes: bytes,
    lock_path: pathlib.Path,
    source_packages: dict[str, RustPackage],
    git_url: str,
    revision: str,
) -> None:
    before = object_value(tomllib.loads(before_bytes.decode()), "original Cargo.lock")
    after = object_value(
        tomllib.loads(lock_path.read_text(encoding="utf-8")), "detached Cargo.lock"
    )
    if {key: value for key, value in before.items() if key != "package"} != {
        key: value for key, value in after.items() if key != "package"
    }:
        raise fail("detached Cargo.lock changed top-level lock metadata")
    before_packages = lock_package_map(before, "original Cargo.lock")
    after_packages = lock_package_map(after, "detached Cargo.lock")
    if set(before_packages) != set(after_packages):
        raise fail("detached Cargo resolution added or removed packages")

    refreshed: set[str] = set()
    for key, original in before_packages.items():
        updated = after_packages[key]
        package_name, package_version = key
        expected = source_packages.get(package_name)
        if expected is None:
            if updated != original:
                raise fail(
                    f"detached Cargo resolution changed {package_name}@{package_version}"
                )
            continue
        if (
            package_version != expected.version
            or "source" in original
            or "checksum" in original
        ):
            raise fail(
                f"recorded Prns package {package_name}@{package_version} is not a path package"
            )
        comparable = copy.deepcopy(updated)
        source = comparable.pop("source", None)
        if comparable != original or not exact_git_source(
            source, git_url, revision, resolved=True
        ):
            raise fail(
                f"detached Cargo resolution changed more than the exact source for "
                f"{package_name}@{package_version}"
            )
        refreshed.add(package_name)
    if refreshed != set(source_packages):
        raise fail(
            "detached Cargo resolution did not refresh every recorded Prns source package"
        )


def cargo_metadata(
    applications_root: pathlib.Path, environment: dict[str, str], *, locked: bool
) -> dict[str, Any]:
    arguments: list[str | os.PathLike[str]] = ["cargo", "metadata"]
    if locked:
        arguments.append("--locked")
    arguments.extend(
        ("--format-version", "1", "--manifest-path", applications_root / "Cargo.toml")
    )
    completed = run(
        arguments,
        cwd=applications_root,
        environment=environment,
        capture=True,
    )
    return object_value(json.loads(completed.stdout), "cargo metadata")


def validate_cargo_sources(
    document: dict[str, Any],
    applications_root: pathlib.Path,
    revision: str,
    git_url: str,
    source_packages: dict[str, RustPackage],
    rewrites: list[CargoRewrite],
) -> None:
    package_entries = document.get("packages")
    if not isinstance(package_entries, list):
        raise fail("cargo metadata packages must be an array")
    sources_by_name: dict[str, dict[str, Any]] = {}
    owners_by_manifest: dict[pathlib.Path, dict[str, Any]] = {}
    for index, entry_value in enumerate(package_entries):
        entry = object_value(entry_value, f"cargo metadata package[{index}]")
        string_value(entry, "id", f"cargo metadata package[{index}]")
        name = entry.get("name")
        if name in source_packages:
            if name in sources_by_name:
                raise fail(
                    f"Cargo metadata contains more than one recorded package named {name}"
                )
            sources_by_name[name] = entry
        manifest_path = entry.get("manifest_path")
        if isinstance(manifest_path, str):
            resolved_manifest = pathlib.Path(manifest_path).resolve()
            if relative_to(resolved_manifest, applications_root):
                owners_by_manifest[resolved_manifest] = entry
    if set(sources_by_name) != set(source_packages):
        missing = sorted(set(source_packages) - set(sources_by_name))
        raise fail(f"Cargo metadata omitted recorded Prns packages: {missing}")
    for name, expected in source_packages.items():
        entry = sources_by_name[name]
        if entry.get("version") != expected.version or not exact_git_source(
            entry.get("source"), git_url, revision, resolved=True
        ):
            raise fail(
                f"{name} resolved from {entry.get('source')}, expected exact Prns revision {revision}"
            )

    resolve = object_value(document.get("resolve"), "cargo metadata resolve")
    raw_nodes = resolve.get("nodes")
    if not isinstance(raw_nodes, list):
        raise fail("cargo metadata resolve.nodes must be an array")
    nodes: dict[str, dict[str, Any]] = {}
    for index, raw_node in enumerate(raw_nodes):
        node = object_value(raw_node, f"cargo metadata resolve.nodes[{index}]")
        nodes[string_value(node, "id", f"cargo metadata resolve.nodes[{index}]")] = node

    for rewrite in rewrites:
        manifest = (applications_root / rewrite.manifest).resolve()
        owner = owners_by_manifest.get(manifest)
        if owner is None:
            raise fail(
                f"Cargo metadata omitted direct dependency owner {rewrite.manifest}"
            )
        raw_dependencies = owner.get("dependencies")
        if not isinstance(raw_dependencies, list):
            raise fail(
                f"Cargo metadata dependencies are missing for {rewrite.manifest}"
            )
        matching_declarations = []
        for raw_dependency in raw_dependencies:
            dependency = object_value(
                raw_dependency, f"Cargo dependency in {rewrite.manifest}"
            )
            alias = dependency.get("rename") or dependency.get("name")
            if (
                alias == rewrite.dependency
                and dependency.get("name") == rewrite.package
            ):
                matching_declarations.append(dependency)
        if not matching_declarations or any(
            not exact_git_source(
                dependency.get("source"), git_url, revision, resolved=False
            )
            for dependency in matching_declarations
        ):
            raise fail(
                f"Cargo metadata did not retain exact direct dependency "
                f"{rewrite.dependency} in {rewrite.manifest}"
            )
        owner_node = nodes.get(
            string_value(owner, "id", f"Cargo package {rewrite.manifest}")
        )
        target_id = string_value(
            sources_by_name[rewrite.package], "id", f"Cargo package {rewrite.package}"
        )
        raw_node_dependencies = (
            owner_node.get("dependencies") if owner_node is not None else None
        )
        if (
            not isinstance(raw_node_dependencies, list)
            or target_id not in raw_node_dependencies
        ):
            raise fail(
                f"Cargo resolve graph omitted direct dependency {rewrite.package} "
                f"from {rewrite.manifest}"
            )


def packed_metadata(artifact: pathlib.Path) -> dict[str, Any]:
    with tarfile.open(artifact, "r:gz") as package:
        package_file = package.extractfile("package/package.json")
        if package_file is None:
            raise fail("personal-rns artifact package.json is unreadable")
        return object_value(json.load(package_file), "packed personal-rns package.json")


def validate_npm_lock_refresh(
    before_bytes: bytes,
    applications_root: pathlib.Path,
    artifact: pathlib.Path,
    artifact_integrity: str,
    selection: str,
    compatibility: dict[str, Any],
) -> None:
    before = object_value(json.loads(before_bytes), "original package-lock.json")
    after = object_value(
        json.loads((applications_root / "package-lock.json").read_bytes()),
        "detached package-lock.json",
    )
    if {key: value for key, value in before.items() if key != "packages"} != {
        key: value for key, value in after.items() if key != "packages"
    }:
        raise fail("detached npm resolution changed top-level lock metadata")
    before_packages = object_value(
        before.get("packages"), "original package-lock packages"
    )
    after_packages = object_value(
        after.get("packages"), "detached package-lock packages"
    )
    prns = object_value(compatibility["prns"], "compatibility.prns")
    javascript = object_value(
        prns["javascriptContract"], "compatibility.prns.javascriptContract"
    )
    package_name = string_value(
        javascript, "package", "compatibility JavaScript contract"
    )
    consumer_keys = {
        pathlib.PurePosixPath(value).parent.as_posix()
        for value in string_array(
            javascript, "consumers", "compatibility JavaScript contract"
        )
    }
    packed = packed_metadata(artifact)
    exact_dependencies: dict[str, str] = {}
    for section in ("dependencies", "optionalDependencies"):
        raw_dependencies = packed.get(section, {})
        dependencies = object_value(raw_dependencies, f"packed personal-rns {section}")
        for dependency, version in dependencies.items():
            if not isinstance(version, str) or SEMVER.fullmatch(version) is None:
                raise fail(
                    f"packed personal-rns {section}.{dependency} must use an exact version"
                )
            exact_dependencies[dependency] = version

    source_entries = {
        key
        for key, value in before_packages.items()
        if key.startswith("../")
        and isinstance(value, dict)
        and value.get("name") == package_name
    }
    if len(source_entries) != 1:
        raise fail(
            "original npm lock must contain one external personal-rns source entry"
        )
    removed = set(before_packages) - set(after_packages)
    if removed != source_entries:
        raise fail(
            f"detached npm resolution removed unexpected lock entries: {sorted(removed)}"
        )
    added = set(after_packages) - set(before_packages)
    expected_added = {
        f"node_modules/{dependency}"
        for dependency in exact_dependencies
        if f"node_modules/{dependency}" not in before_packages
    }
    if added != expected_added:
        raise fail(
            f"detached npm resolution added unexpected lock entries: {sorted(added)}"
        )

    allowed_changes = consumer_keys | {f"node_modules/{package_name}"}
    for key in set(before_packages) & set(after_packages):
        if key in allowed_changes:
            continue
        if before_packages[key] != after_packages[key]:
            raise fail(f"detached npm resolution changed existing lock entry {key}")
    for consumer in consumer_keys:
        expected_consumer = copy.deepcopy(
            object_value(
                before_packages.get(consumer), f"original npm consumer {consumer}"
            )
        )
        dependencies = object_value(
            expected_consumer.get("dependencies"),
            f"original npm consumer {consumer} dependencies",
        )
        if dependencies.get(package_name) is None:
            raise fail(f"original npm consumer {consumer} omits {package_name}")
        dependencies[package_name] = selection
        if after_packages.get(consumer) != expected_consumer:
            raise fail(
                f"detached npm consumer lock changed beyond {package_name}: {consumer}"
            )
    installed_lock = object_value(
        after_packages.get(f"node_modules/{package_name}"),
        f"detached npm lock node_modules/{package_name}",
    )
    if (
        installed_lock.get("integrity") != artifact_integrity
        or artifact.name not in str(installed_lock.get("resolved"))
        or installed_lock.get("version") != packed.get("version")
        or installed_lock.get("dependencies") != packed.get("dependencies")
        or installed_lock.get("optionalDependencies")
        != packed.get("optionalDependencies")
    ):
        raise fail(
            "detached npm lock does not exactly describe the packed personal-rns artifact"
        )
    for dependency, version in exact_dependencies.items():
        entry = object_value(
            after_packages.get(f"node_modules/{dependency}"),
            f"detached npm dependency {dependency}",
        )
        if entry.get("version") != version:
            raise fail(
                f"detached npm dependency {dependency} did not resolve exact version {version}"
            )


def validate_npm_resolution(
    applications_root: pathlib.Path,
    artifact: pathlib.Path,
    artifact_digest: str,
    artifact_integrity: str,
) -> None:
    artifact_bytes = artifact.read_bytes()
    if hashlib.sha256(artifact_bytes).hexdigest() != artifact_digest:
        raise fail(
            "personal-rns artifact changed after its compatibility digest was checked"
        )
    actual_integrity = (
        "sha512-" + base64.b64encode(hashlib.sha512(artifact_bytes).digest()).decode()
    )
    if actual_integrity != artifact_integrity:
        raise fail("personal-rns artifact changed after its npm integrity was recorded")
    installed = applications_root / "node_modules" / "personal-rns"
    if not installed.is_dir() or installed.is_symlink():
        raise fail("personal-rns must be installed as an unpacked detached artifact")
    package = object_value(
        json.loads((installed / "package.json").read_text(encoding="utf-8")),
        "installed personal-rns package.json",
    )
    exports = object_value(package.get("exports"), "installed personal-rns exports")
    if "./contract" not in exports:
        raise fail("installed personal-rns artifact does not expose ./contract")
    lock_text = (applications_root / "package-lock.json").read_text(encoding="utf-8")
    if "../prns-js" in lock_text or os.fspath(REPOSITORY_ROOT) in lock_text:
        raise fail("detached npm lock still references the enclosing Prns checkout")
    if artifact.name not in lock_text:
        raise fail("detached npm lock does not name the packed personal-rns artifact")
    if artifact_integrity not in lock_text:
        raise fail(
            "detached npm lock does not enforce the packed personal-rns integrity"
        )


def qualify(
    repository_root: pathlib.Path,
    git_url: str,
    keep_workspace: pathlib.Path | None,
) -> None:
    compatibility = load_compatibility()
    prns = object_value(compatibility["prns"], "compatibility.prns")
    revision = string_value(prns, "revision", "compatibility.prns")
    source_head = git_output(repository_root, "rev-parse", "HEAD")
    require_qualification_toolchains(compatibility, repository_root)
    require_clean_application_source(repository_root)
    reject_tracked_symlinks(repository_root)
    reject_resolver_configuration(APPLICATIONS_ROOT)
    cargo_rewrites = cargo_rewrite_plan(
        APPLICATIONS_ROOT, repository_root, compatibility
    )
    npm_rewrites = npm_rewrite_plan(APPLICATIONS_ROOT, repository_root, compatibility)
    _, source_packages = rust_packages(compatibility)
    before_fingerprint = tracked_base_fingerprint(repository_root)
    before_status = base_status(repository_root)

    temporary: tempfile.TemporaryDirectory[str] | None = None
    if keep_workspace is None:
        temporary = tempfile.TemporaryDirectory(prefix="prns-app-detached-")
        work = pathlib.Path(temporary.name)
    else:
        work = keep_workspace.resolve()
        if work.exists():
            raise fail(f"--keep-workspace destination already exists: {work}")
        work.mkdir(parents=True)
    try:
        applications_root = export_applications(repository_root, work)
        environment = controlled_environment(work, applications_root)
        prns_root = work / "prns-source"
        checkout_prns(git_url, revision, prns_root, environment)
        run(
            (
                sys.executable,
                applications_root / "ci" / "check_compatibility.py",
                "--prns-root",
                prns_root,
                "--require-head",
            ),
            cwd=applications_root,
            environment=environment,
        )
        artifact, artifact_digest, artifact_integrity = build_javascript_artifact(
            prns_root, applications_root, compatibility, environment
        )
        original_cargo_lock = (applications_root / "Cargo.lock").read_bytes()
        original_npm_lock = (applications_root / "package-lock.json").read_bytes()
        rewrite_cargo_dependencies(applications_root, cargo_rewrites, git_url, revision)
        personal_rns_spec = rewrite_npm_dependencies(
            applications_root, npm_rewrites, artifact
        )
        reject_external_cargo_paths(applications_root)
        reject_external_npm_paths(applications_root)

        environment["LXMF_VENV"] = os.fspath(
            applications_root / "target" / "interop" / "lxmf-1.1.0"
        )
        environment["PRNS_COMPATIBILITY_PRNS_ROOT"] = os.fspath(prns_root)
        cargo_metadata(applications_root, environment, locked=False)
        validate_cargo_lock_refresh(
            original_cargo_lock,
            applications_root / "Cargo.lock",
            source_packages,
            git_url,
            revision,
        )
        locked_metadata = cargo_metadata(applications_root, environment, locked=True)
        validate_cargo_sources(
            locked_metadata,
            applications_root,
            revision,
            git_url,
            source_packages,
            cargo_rewrites,
        )
        run(
            (
                "npm",
                "install",
                "--package-lock-only",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
            ),
            cwd=applications_root,
            environment=environment,
        )
        validate_npm_lock_refresh(
            original_npm_lock,
            applications_root,
            artifact,
            artifact_integrity,
            personal_rns_spec,
            compatibility,
        )
        reject_external_npm_lock_paths(applications_root)
        run(
            ("npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"),
            cwd=applications_root,
            environment=environment,
        )
        validate_npm_resolution(
            applications_root, artifact, artifact_digest, artifact_integrity
        )

        generated = applications_root / "sdk" / "expo" / "src" / "contract.generated.ts"
        expected_generated = generated.read_bytes()
        run(
            ("npm", "run", "api:generate"),
            cwd=applications_root,
            environment=environment,
        )
        if generated.read_bytes() != expected_generated:
            raise fail(
                "detached contract generation differs from the tracked application artifact"
            )
        run(("npm", "run", "verify"), cwd=applications_root, environment=environment)
        run(
            ("npm", "run", "lxmf:verify"),
            cwd=applications_root,
            environment=environment,
        )
        print(
            "APPLICATION_DETACHED_MOBILITY_OK "
            f"application={source_head} prns={revision} "
            f"personal_rns_sha256={artifact_digest}",
            flush=True,
        )
    finally:
        try:
            after_fingerprint = tracked_base_fingerprint(repository_root)
            after_status = base_status(repository_root)
            if before_fingerprint != after_fingerprint or before_status != after_status:
                raise fail(
                    "detached qualification changed base Prns source or generated output"
                )
        finally:
            if temporary is not None:
                temporary.cleanup()


def arguments() -> argparse.Namespace:
    compatibility = load_compatibility()
    prns = object_value(compatibility["prns"], "compatibility.prns")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--prns-git-url",
        default=os.environ.get("PRNS_MOBILITY_GIT_URL")
        or string_value(prns, "repository", "compatibility.prns"),
        help="Git repository containing the exact recorded Prns revision",
    )
    parser.add_argument(
        "--keep-workspace",
        type=pathlib.Path,
        help="retain the detached workspace at this new path for inspection",
    )
    return parser.parse_args()


def main() -> int:
    selected = arguments()
    qualify(REPOSITORY_ROOT, selected.prns_git_url, selected.keep_workspace)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (
        QualificationFailure,
        OSError,
        subprocess.CalledProcessError,
        tomllib.TOMLDecodeError,
    ) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1) from error
