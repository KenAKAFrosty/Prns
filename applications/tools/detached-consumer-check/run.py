#!/usr/bin/env python3
"""Export and qualify the application workspace against one exact Prns revision."""

from __future__ import annotations

import argparse
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
from dataclasses import dataclass
from typing import Any, Iterable


APPLICATIONS_ROOT = pathlib.Path(__file__).resolve().parents[2]
REPOSITORY_ROOT = APPLICATIONS_ROOT.parent
COMPATIBILITY_PATH = APPLICATIONS_ROOT / "release" / "compatibility.json"
REVISION = re.compile(r"[0-9a-f]{40}\Z")
INLINE_DEPENDENCY = re.compile(
    r'^(?P<prefix>\s*(?P<name>[A-Za-z0-9_-]+)\s*=\s*\{)(?P<body>.*)(?P<suffix>\}\s*(?:#.*)?)$'
)
PATH_ATTRIBUTE = re.compile(r'(?<![A-Za-z0-9_-])path\s*=\s*"(?P<path>[^"]+)"')
PACKAGE_ATTRIBUTE = re.compile(r'(?<![A-Za-z0-9_-])package\s*=\s*"(?P<package>[^"]+)"')
DEPENDENCY_SECTIONS = (
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
)
BASE_CARGO_PACKAGES = frozenset(
    {"personal-rns", "prns-core", "prns-host", "prns-host-snapshot"}
)


class QualificationFailure(RuntimeError):
    """A detached qualification invariant was not met."""


@dataclass(frozen=True)
class CargoRewrite:
    manifest: pathlib.Path
    line_index: int
    original: str
    package: str


@dataclass(frozen=True)
class NpmRewrite:
    manifest: pathlib.Path
    section: str
    dependency: str
    original: str


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


def object_value(value: Any, owner: str) -> dict[str, Any]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise fail(f"{owner} must be a JSON object")
    return value


def string_value(owner: dict[str, Any], key: str, path: str) -> str:
    value = owner.get(key)
    if not isinstance(value, str) or not value:
        raise fail(f"{path}.{key} must be a non-empty string")
    return value


def load_compatibility() -> dict[str, Any]:
    document = object_value(
        json.loads(COMPATIBILITY_PATH.read_text(encoding="utf-8")),
        str(COMPATIBILITY_PATH),
    )
    if document.get("schemaVersion") != 1:
        raise fail("release/compatibility.json schemaVersion must be 1")
    prns = object_value(document.get("prns"), "compatibility.prns")
    revision = string_value(prns, "revision", "compatibility.prns")
    if REVISION.fullmatch(revision) is None:
        raise fail("compatibility.prns.revision must be a full lowercase Git commit ID")
    return document


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
        raise fail("applications/ must be committed before its tracked export is qualified")


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


def export_applications(repository_root: pathlib.Path, destination: pathlib.Path) -> pathlib.Path:
    archive = destination / "applications.tar"
    run(
        ("git", "-C", repository_root, "archive", "--format=tar", "-o", archive, "HEAD", "applications"),
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


def cargo_package_name(manifest: pathlib.Path) -> str:
    document = tomllib.loads(manifest.read_text(encoding="utf-8"))
    package = document.get("package")
    if not isinstance(package, dict) or not isinstance(package.get("name"), str):
        raise fail(f"{manifest} does not declare package.name")
    return package["name"]


def manifests_below(root: pathlib.Path, filename: str) -> Iterable[pathlib.Path]:
    excluded = {"node_modules", "target", "vendor", "__pycache__"}
    for path in sorted(root.rglob(filename)):
        relative = path.relative_to(root)
        if not excluded.intersection(relative.parts):
            yield path


def cargo_rewrite_plan(
    applications_root: pathlib.Path, repository_root: pathlib.Path
) -> list[CargoRewrite]:
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
                raise fail(f"Cargo dependency escapes the Prns repository: {manifest}:{line_index + 1}")
            dependency_manifest = dependency_path / "Cargo.toml"
            if not dependency_manifest.is_file():
                raise fail(f"Cargo dependency has no manifest: {dependency_path}")
            package_match = PACKAGE_ATTRIBUTE.search(declaration.group("body"))
            expected_package = (
                package_match.group("package")
                if package_match is not None
                else declaration.group("name")
            )
            actual_package = cargo_package_name(dependency_manifest)
            if actual_package != expected_package or actual_package not in BASE_CARGO_PACKAGES:
                raise fail(
                    f"unreviewed external Cargo dependency {expected_package} at "
                    f"{manifest}:{line_index + 1}"
                )
            result.append(
                CargoRewrite(
                    manifest=manifest.relative_to(applications_root),
                    line_index=line_index,
                    original=path_match.group("path"),
                    package=actual_package,
                )
            )
    if {rewrite.package for rewrite in result} != BASE_CARGO_PACKAGES:
        raise fail("external Cargo dependency inventory does not match the application contract")
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
                rf'(?<![A-Za-z0-9_-])path\s*=\s*{re.escape(json.dumps(rewrite.original))}'
            )
            updated, count = path_expression.subn(replacement, lines[rewrite.line_index], count=1)
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
                raise fail(f"Cargo path escapes detached applications/: {manifest}: {path_value}")


def npm_rewrite_plan(
    applications_root: pathlib.Path, repository_root: pathlib.Path
) -> list[NpmRewrite]:
    result = []
    for manifest in manifests_below(applications_root, "package.json"):
        package = object_value(json.loads(manifest.read_text(encoding="utf-8")), str(manifest))
        for section in DEPENDENCY_SECTIONS:
            dependencies = package.get(section)
            if dependencies is None:
                continue
            dependencies = object_value(dependencies, f"{manifest}:{section}")
            for dependency, selection in dependencies.items():
                if not isinstance(selection, str) or not selection.startswith("file:"):
                    continue
                dependency_path = (manifest.parent / selection.removeprefix("file:")).resolve()
                if relative_to(dependency_path, applications_root):
                    continue
                if not relative_to(dependency_path, repository_root):
                    raise fail(f"npm dependency escapes the Prns repository: {manifest}:{dependency}")
                dependency_manifest = dependency_path / "package.json"
                if not dependency_manifest.is_file():
                    raise fail(f"npm dependency has no package.json: {dependency_path}")
                dependency_package = object_value(
                    json.loads(dependency_manifest.read_text(encoding="utf-8")),
                    str(dependency_manifest),
                )
                if dependency != "personal-rns" or dependency_package.get("name") != dependency:
                    raise fail(f"unreviewed external npm dependency {dependency} in {manifest}")
                result.append(
                    NpmRewrite(
                        manifest=manifest.relative_to(applications_root),
                        section=section,
                        dependency=dependency,
                        original=selection,
                    )
                )
    if len(result) != 2:
        raise fail(f"expected two personal-rns npm path dependencies, found {len(result)}")
    return result


def rewrite_npm_dependencies(
    applications_root: pathlib.Path,
    rewrites: list[NpmRewrite],
    artifact: pathlib.Path,
) -> str:
    selections = set()
    for rewrite in rewrites:
        manifest = applications_root / rewrite.manifest
        package = object_value(json.loads(manifest.read_text(encoding="utf-8")), str(manifest))
        dependencies = object_value(package[rewrite.section], f"{manifest}:{rewrite.section}")
        if dependencies.get(rewrite.dependency) != rewrite.original:
            raise fail(f"npm dependency changed before rewrite: {manifest}:{rewrite.dependency}")
        relative_artifact = os.path.relpath(artifact, manifest.parent)
        selection = f"file:{pathlib.PurePath(relative_artifact).as_posix()}"
        dependencies[rewrite.dependency] = selection
        manifest.write_text(f"{json.dumps(package, indent=2)}\n", encoding="utf-8")
        selections.add(selection)
    if len(selections) != 1:
        raise fail("detached npm workspaces must use one shared personal-rns artifact selection")
    return selections.pop()


def reject_external_npm_paths(applications_root: pathlib.Path) -> None:
    for manifest in manifests_below(applications_root, "package.json"):
        package = object_value(json.loads(manifest.read_text(encoding="utf-8")), str(manifest))
        for section in DEPENDENCY_SECTIONS:
            dependencies = package.get(section)
            if dependencies is None:
                continue
            for dependency, selection in object_value(
                dependencies, f"{manifest}:{section}"
            ).items():
                if isinstance(selection, str) and selection.startswith("file:"):
                    resolved = (manifest.parent / selection.removeprefix("file:")).resolve()
                    if not relative_to(resolved, applications_root):
                        raise fail(
                            f"npm file dependency escapes detached applications/: "
                            f"{manifest}:{dependency}"
                        )


def checkout_prns(git_url: str, revision: str, destination: pathlib.Path) -> None:
    destination.mkdir()
    run(("git", "init", "--quiet"), cwd=destination)
    run(("git", "remote", "add", "origin", git_url), cwd=destination)
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
    )
    run(("git", "checkout", "--quiet", "--detach", "FETCH_HEAD"), cwd=destination)
    resolved = git_output(destination, "rev-parse", "HEAD")
    if resolved != revision:
        raise fail(f"Prns checkout resolved {resolved}, expected {revision}")


def build_javascript_artifact(
    prns_root: pathlib.Path,
    applications_root: pathlib.Path,
    compatibility: dict[str, Any],
) -> tuple[pathlib.Path, str]:
    prns = object_value(compatibility["prns"], "compatibility.prns")
    javascript = object_value(prns["javascriptContract"], "compatibility.prns.javascriptContract")
    source = prns_root / string_value(
        javascript, "sourcePath", "compatibility.prns.javascriptContract"
    )
    run(("npm", "ci", "--ignore-scripts"), cwd=source)
    run(("npm", "run", "build:code"), cwd=source)
    vendor = applications_root / "vendor"
    vendor.mkdir()
    packed = run(
        ("npm", "pack", "--json", "--pack-destination", vendor),
        cwd=source,
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

    required = {
        "package/package.json",
        "package/dist/contract.js",
        "package/dist/contract.d.ts",
        "package/dist-cjs/contract.js",
    }
    with tarfile.open(artifact, "r:gz") as package:
        names = set()
        for member in package.getmembers():
            path = pathlib.PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or member.issym() or member.islnk():
                raise fail(f"personal-rns artifact contains an unsafe path: {member.name}")
            names.add(member.name)
        missing = required - names
        if missing:
            raise fail(f"personal-rns artifact is missing: {', '.join(sorted(missing))}")
        package_file = package.extractfile("package/package.json")
        if package_file is None:
            raise fail("personal-rns artifact package.json is unreadable")
        metadata = object_value(json.load(package_file), "packed personal-rns package.json")
    expected_name = string_value(javascript, "package", "compatibility.prns.javascriptContract")
    expected_version = string_value(javascript, "version", "compatibility.prns.javascriptContract")
    if metadata.get("name") != expected_name or metadata.get("version") != expected_version:
        raise fail(f"packed artifact must identify {expected_name}@{expected_version}")
    exports = object_value(metadata.get("exports"), "packed personal-rns exports")
    subpath = string_value(javascript, "subpath", "compatibility.prns.javascriptContract")
    if subpath not in exports:
        raise fail(f"packed personal-rns artifact does not export {subpath}")
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    return artifact, digest


def validate_cargo_sources(
    applications_root: pathlib.Path, revision: str, packages: frozenset[str]
) -> None:
    metadata = run(
        (
            "cargo",
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
            applications_root / "Cargo.toml",
        ),
        cwd=applications_root,
        capture=True,
    )
    document = object_value(json.loads(metadata.stdout), "cargo metadata")
    found: dict[str, str] = {}
    package_entries = document.get("packages")
    if not isinstance(package_entries, list):
        raise fail("cargo metadata packages must be an array")
    for entry_value in package_entries:
        entry = object_value(entry_value, "cargo metadata package")
        name = entry.get("name")
        if name not in packages:
            continue
        source = entry.get("source")
        if not isinstance(source, str):
            raise fail(f"{name} did not resolve from the exact Prns Git source")
        found[name] = source
    if set(found) != set(packages):
        raise fail(f"Cargo metadata omitted pinned packages: {sorted(packages - set(found))}")
    for package, source in found.items():
        if not source.startswith("git+") or not source.endswith(f"#{revision}"):
            raise fail(f"{package} resolved from {source}, expected Git revision {revision}")


def validate_npm_resolution(applications_root: pathlib.Path, artifact: pathlib.Path) -> None:
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


def qualify(
    repository_root: pathlib.Path,
    git_url: str,
    keep_workspace: pathlib.Path | None,
) -> None:
    compatibility = load_compatibility()
    prns = object_value(compatibility["prns"], "compatibility.prns")
    revision = string_value(prns, "revision", "compatibility.prns")
    source_head = git_output(repository_root, "rev-parse", "HEAD")
    require_clean_application_source(repository_root)
    reject_tracked_symlinks(repository_root)
    cargo_rewrites = cargo_rewrite_plan(APPLICATIONS_ROOT, repository_root)
    npm_rewrites = npm_rewrite_plan(APPLICATIONS_ROOT, repository_root)
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
        prns_root = work / "prns-source"
        checkout_prns(git_url, revision, prns_root)
        run(
            (
                sys.executable,
                applications_root / "ci" / "check_compatibility.py",
                "--prns-root",
                prns_root,
                "--require-head",
            ),
            cwd=applications_root,
        )
        artifact, artifact_digest = build_javascript_artifact(
            prns_root, applications_root, compatibility
        )
        rewrite_cargo_dependencies(applications_root, cargo_rewrites, git_url, revision)
        personal_rns_spec = rewrite_npm_dependencies(
            applications_root, npm_rewrites, artifact
        )
        (applications_root / "Cargo.lock").unlink()
        (applications_root / "package-lock.json").unlink()
        reject_external_cargo_paths(applications_root)
        reject_external_npm_paths(applications_root)

        environment = os.environ.copy()
        environment["CARGO_NET_GIT_FETCH_WITH_CLI"] = "true"
        environment["LXMF_VENV"] = os.fspath(
            applications_root / "target" / "interop" / "lxmf-1.1.0"
        )
        environment["PRNS_COMPATIBILITY_PRNS_ROOT"] = os.fspath(prns_root)
        environment["PRNS_PERSONAL_RNS_SPEC"] = personal_rns_spec
        run(
            ("cargo", "generate-lockfile", "--manifest-path", applications_root / "Cargo.toml"),
            cwd=applications_root,
            environment=environment,
        )
        validate_cargo_sources(applications_root, revision, BASE_CARGO_PACKAGES)
        run(("npm", "install", "--package-lock-only", "--ignore-scripts"), cwd=applications_root)
        run(("npm", "ci", "--ignore-scripts"), cwd=applications_root)
        validate_npm_resolution(applications_root, artifact)

        generated = applications_root / "sdk" / "expo" / "src" / "contract.generated.ts"
        expected_generated = generated.read_bytes()
        run(("npm", "run", "api:generate"), cwd=applications_root, environment=environment)
        if generated.read_bytes() != expected_generated:
            raise fail("detached contract generation differs from the tracked application artifact")
        run(("npm", "run", "verify"), cwd=applications_root, environment=environment)
        run(("npm", "run", "lxmf:verify"), cwd=applications_root, environment=environment)
        print(
            "APPLICATION_DETACHED_MOBILITY_OK "
            f"application={source_head} prns={revision} "
            f"personal_rns_sha256={artifact_digest}",
            flush=True,
        )
    finally:
        after_fingerprint = tracked_base_fingerprint(repository_root)
        after_status = base_status(repository_root)
        if temporary is not None:
            temporary.cleanup()
        if before_fingerprint != after_fingerprint or before_status != after_status:
            raise fail("detached qualification changed base Prns source or generated output")


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
    except (QualificationFailure, OSError, subprocess.CalledProcessError, tomllib.TOMLDecodeError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1) from error
