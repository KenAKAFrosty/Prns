from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tarfile
import tempfile
import urllib.error
import urllib.request
import zipfile
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Callable

from validation.hardening.embedded_host import HostPlatform
from validation.hardening.embedded_isa.contract import (
    Architecture,
    HostedPackages,
    InventoryError as IsaInventoryError,
    SourceArchive,
    load_inventory as load_isa_inventory,
)
from validation.hardening.embedded_platform.contract import (
    ArchitectureQemuExecution,
    InventoryError as PlatformInventoryError,
    RenodeExecution,
    load_inventory as load_platform_inventory,
)
from validation.hardening.embedded_platform.discovery import file_sha256
from validation.hardening.embedded_readiness.system import SystemProbe


Download = Callable[[str, Path], None]
Run = Callable[[tuple[str, ...], Path], None]


class PreparationError(RuntimeError):
    pass


@dataclass(frozen=True)
class Archive:
    source_url: str
    source_sha256: str


@dataclass(frozen=True)
class SourceBuild:
    archive: Archive


@dataclass(frozen=True)
class HostedArchive:
    archive: Archive


Acquisition = SourceBuild | HostedArchive


class IdentityScope(Enum):
    FIRST_LINE = "first-line"
    ALL_LINES = "all-lines"


class ArchivePurpose(Enum):
    HOSTED_PACKAGE = "hosted-package"
    SOURCE_BUILD = "source-build"


@dataclass(frozen=True)
class ModelRequirement:
    path: str
    sha256: str


@dataclass(frozen=True)
class EmulatorRequirement:
    executable: str
    identity: tuple[str, ...]
    identity_scope: IdentityScope
    acquisition: Acquisition
    model: ModelRequirement | None


@dataclass(frozen=True)
class PreparationOutcome:
    root: Path
    executables: tuple[Path, ...]


def requirements_for_suites(
    suites: tuple[str, ...], host: HostPlatform
) -> tuple[EmulatorRequirement, ...]:
    if not suites:
        raise PreparationError("at least one embedded assurance suite is required")
    isa = load_isa_inventory()
    platforms = load_platform_inventory()
    requirements = []
    for suite in suites:
        architecture = next(
            (entry for entry in isa.architectures if entry.suite == suite), None
        )
        if architecture is not None:
            requirements.append(architecture_requirement(architecture, host))
            continue
        platform = next(
            (entry for entry in platforms.platforms if entry.suite == suite), None
        )
        if platform is None:
            raise PreparationError(f"unknown embedded assurance suite {suite!r}")
        match platform.execution:
            case ArchitectureQemuExecution():
                architecture = isa.architecture_named(platform.architecture)
                requirements.append(architecture_requirement(architecture, host))
            case RenodeExecution() as execution:
                package = execution.emulator.package_for_host(host)
                if package is None:
                    raise PreparationError(
                        f"{suite} has no pinned emulator package for {host.value}"
                    )
                requirements.append(
                    EmulatorRequirement(
                        executable=execution.emulator.executable,
                        identity=execution.emulator.identity,
                        identity_scope=IdentityScope.ALL_LINES,
                        acquisition=HostedArchive(
                            Archive(package.source_url, package.source_sha256)
                        ),
                        model=ModelRequirement(
                            execution.platform_description,
                            execution.platform_description_sha256,
                        ),
                    )
                )
    unique = {}
    for requirement in requirements:
        existing = unique.get(requirement.executable)
        if existing is not None and existing != requirement:
            raise PreparationError(
                f"emulator {requirement.executable!r} has conflicting requirements"
            )
        unique[requirement.executable] = requirement
    return tuple(unique[name] for name in sorted(unique))


def architecture_requirement(
    architecture: Architecture, host: HostPlatform
) -> EmulatorRequirement:
    acquisition = architecture.emulator.acquisition
    match acquisition:
        case SourceArchive(source_url=url, source_sha256=checksum):
            resolved = SourceBuild(Archive(url, checksum))
        case HostedPackages():
            package = acquisition.for_host(host)
            resolved = HostedArchive(
                Archive(package.source_url, package.source_sha256)
            )
    return EmulatorRequirement(
        executable=architecture.emulator.executable,
        identity=(architecture.emulator.identity.banner,),
        identity_scope=IdentityScope.FIRST_LINE,
        acquisition=resolved,
        model=None,
    )


def prepare(
    root: Path,
    suites: tuple[str, ...],
    *,
    probe: SystemProbe | None = None,
    download: Download | None = None,
    run: Run | None = None,
) -> PreparationOutcome:
    try:
        return prepare_checked(root, suites, probe, download, run)
    except PreparationError:
        raise
    except (
        IsaInventoryError,
        PlatformInventoryError,
        OSError,
        tarfile.TarError,
        zipfile.BadZipFile,
    ) as error:
        raise PreparationError(
            f"could not prepare embedded assurance tools: {error}"
        ) from error


def prepare_checked(
    root: Path,
    suites: tuple[str, ...],
    probe: SystemProbe | None,
    download: Download | None,
    run: Run | None,
) -> PreparationOutcome:
    probe = probe or SystemProbe()
    host = probe.host_platform()
    if host is None:
        raise PreparationError("this host has no embedded assurance acquisition contract")
    destination = safe_root(root)
    destination.mkdir(parents=True, exist_ok=True)
    requirements = requirements_for_suites(suites, host)
    downloader = download or download_file
    runner = run or run_command
    groups = acquisition_groups(requirements)
    installed = {}
    for acquisition, members in groups:
        if isinstance(acquisition, SourceBuild):
            paths = install_source(destination, acquisition, members, downloader, runner)
        else:
            paths = install_hosted(destination, acquisition, members, downloader)
        installed.update(paths)
    bin_directory = destination / "bin"
    bin_directory.mkdir(exist_ok=True)
    linked = []
    for requirement in requirements:
        executable = installed[requirement.executable]
        verify_identity(
            executable, requirement.identity, requirement.identity_scope
        )
        if requirement.model is not None:
            verify_model(executable, requirement.model)
        link = bin_directory / requirement.executable
        replace_link(link, executable)
        linked.append(link)
    return PreparationOutcome(destination, tuple(linked))


def safe_root(root: Path) -> Path:
    resolved = root.expanduser().resolve()
    if resolved == Path(resolved.anchor) or resolved == Path.home().resolve():
        raise PreparationError(f"refusing broad preparation root {resolved}")
    return resolved


def acquisition_groups(
    requirements: tuple[EmulatorRequirement, ...],
) -> tuple[tuple[Acquisition, tuple[EmulatorRequirement, ...]], ...]:
    grouped: dict[Acquisition, list[EmulatorRequirement]] = {}
    for requirement in requirements:
        grouped.setdefault(requirement.acquisition, []).append(requirement)
    return tuple(
        (acquisition, tuple(members))
        for acquisition, members in sorted(
            grouped.items(), key=lambda item: item[0].archive.source_sha256
        )
    )


def install_hosted(
    root: Path,
    acquisition: HostedArchive,
    requirements: tuple[EmulatorRequirement, ...],
    download: Download,
) -> dict[str, Path]:
    destination = root / "packages" / acquisition.archive.source_sha256
    if not destination.is_dir():
        archive = acquire(root, acquisition.archive, download)
        with tempfile.TemporaryDirectory(dir=root) as temporary:
            staging = Path(temporary) / "package"
            staging.mkdir()
            extract_archive(archive, staging, ArchivePurpose.HOSTED_PACKAGE)
            destination.parent.mkdir(parents=True, exist_ok=True)
            staging.rename(destination)
    return locate_executables(destination, requirements)


def install_source(
    root: Path,
    acquisition: SourceBuild,
    requirements: tuple[EmulatorRequirement, ...],
    download: Download,
    run: Run,
) -> dict[str, Path]:
    destination = root / "builds" / acquisition.archive.source_sha256
    existing = existing_executables(destination, requirements)
    if len(existing) == len(requirements):
        return existing
    if destination.exists():
        raise PreparationError(f"incomplete emulator build at {destination}")
    archive = acquire(root, acquisition.archive, download)
    with tempfile.TemporaryDirectory(dir=root) as temporary:
        temporary_path = Path(temporary)
        sources = temporary_path / "sources"
        sources.mkdir()
        extract_archive(archive, sources, ArchivePurpose.SOURCE_BUILD)
        source = source_root(sources)
        build = temporary_path / "build"
        build.mkdir()
        install = temporary_path / "install"
        targets = ",".join(
            f"{requirement.executable.removeprefix('qemu-system-')}-softmmu"
            for requirement in requirements
        )
        run(
            (
                str(source / "configure"),
                f"--prefix={install}",
                f"--target-list={targets}",
                "--disable-docs",
                "--disable-werror",
            ),
            build,
        )
        run(("ninja", "install"), build)
        destination.parent.mkdir(parents=True, exist_ok=True)
        install.rename(destination)
    return locate_executables(destination, requirements)


def acquire(root: Path, archive: Archive, download: Download) -> Path:
    downloads = root / "downloads"
    downloads.mkdir(exist_ok=True)
    suffix = archive_suffix(archive.source_url)
    destination = downloads / f"{archive.source_sha256}{suffix}"
    if not destination.is_file():
        temporary = destination.with_suffix(destination.suffix + ".partial")
        download(archive.source_url, temporary)
        if file_sha256(temporary) != archive.source_sha256:
            temporary.unlink(missing_ok=True)
            raise PreparationError(
                f"downloaded archive checksum does not match {archive.source_sha256}"
            )
        temporary.rename(destination)
    elif file_sha256(destination) != archive.source_sha256:
        raise PreparationError(f"cached archive checksum mismatch at {destination}")
    return destination


def archive_suffix(url: str) -> str:
    for suffix in (".tar.xz", ".tar.gz", ".zip"):
        if url.endswith(suffix):
            return suffix
    raise PreparationError(f"automated preparation does not support archive {url!r}")


def download_file(url: str, destination: Path) -> None:
    request = urllib.request.Request(url, headers={"User-Agent": "prns-validation"})
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            with destination.open("wb") as output:
                shutil.copyfileobj(response, output)
    except (OSError, urllib.error.URLError) as error:
        raise PreparationError(f"could not download {url}: {error}") from error


def extract_archive(
    archive: Path,
    destination: Path,
    purpose: ArchivePurpose = ArchivePurpose.HOSTED_PACKAGE,
) -> None:
    if archive.name.endswith((".tar.xz", ".tar.gz")):
        with tarfile.open(archive) as source:
            members = source.getmembers()
            extractable = []
            for member in members:
                validate_archive_path(destination, member.name)
                if member.isdev():
                    raise PreparationError(f"archive contains device {member.name!r}")
                if member.issym():
                    if (
                        purpose is ArchivePurpose.SOURCE_BUILD
                        and Path(member.linkname).is_absolute()
                    ):
                        continue
                    validate_archive_path(
                        destination, str(Path(member.name).parent / member.linkname)
                    )
                elif member.islnk():
                    validate_archive_path(destination, member.linkname)
                extractable.append(member)
            source.extractall(destination, extractable)
        return
    if archive.name.endswith(".zip"):
        with zipfile.ZipFile(archive) as source:
            for member in source.infolist():
                validate_archive_path(destination, member.filename)
                mode = member.external_attr >> 16
                if stat.S_ISLNK(mode):
                    raise PreparationError(
                        f"zip archive contains symbolic link {member.filename!r}"
                    )
            source.extractall(destination)
        return
    raise PreparationError(f"unsupported archive format: {archive}")


def validate_archive_path(root: Path, member: str) -> None:
    resolved_root = root.resolve()
    path = (resolved_root / member).resolve()
    if path != resolved_root and resolved_root not in path.parents:
        raise PreparationError(f"archive member escapes destination: {member!r}")


def source_root(directory: Path) -> Path:
    candidates = tuple(
        path
        for path in (directory, *tuple(directory.iterdir()))
        if path.is_dir() and (path / "configure").is_file()
    )
    if len(candidates) != 1:
        raise PreparationError(
            f"source archive contains {len(candidates)} configure entrypoints"
        )
    return candidates[0]


def locate_executables(
    root: Path,
    requirements: tuple[EmulatorRequirement, ...],
) -> dict[str, Path]:
    if not root.is_dir():
        raise PreparationError(f"emulator installation is missing: {root}")
    located = existing_executables(root, requirements)
    for requirement in requirements:
        if requirement.executable not in located:
            count = executable_count(root, requirement.executable)
            raise PreparationError(
                f"{root} contains {count} executable {requirement.executable!r} files"
            )
    return located


def existing_executables(
    root: Path, requirements: tuple[EmulatorRequirement, ...]
) -> dict[str, Path]:
    if not root.is_dir():
        return {}
    located = {}
    for requirement in requirements:
        matches = tuple(
            path.resolve()
            for path in root.rglob(requirement.executable)
            if path.is_file() and os.access(path, os.X_OK)
        )
        if len(matches) == 1:
            located[requirement.executable] = matches[0]
    return located


def executable_count(root: Path, executable: str) -> int:
    return sum(
        path.is_file() and os.access(path, os.X_OK)
        for path in root.rglob(executable)
    )


def verify_identity(
    executable: Path, expected: tuple[str, ...], scope: IdentityScope
) -> None:
    try:
        result = subprocess.run(
            (str(executable), "--version"),
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise PreparationError(f"could not inspect {executable}: {error}") from error
    actual = tuple(
        line.strip()
        for line in f"{result.stdout}\n{result.stderr}".splitlines()
        if line.strip()
    )
    if scope is IdentityScope.FIRST_LINE:
        actual = actual[:1]
    if result.returncode != 0 or actual != expected:
        raise PreparationError(
            f"emulator identity at {executable} is {actual!r}, expected {expected!r}"
        )


def verify_model(executable: Path, model: ModelRequirement) -> None:
    roots = (executable.parent, executable.parent.parent, executable.parent.parent / "libexec")
    candidates = tuple(root / model.path for root in roots)
    matched = tuple(path for path in candidates if path.is_file())
    if len(matched) != 1:
        raise PreparationError(
            f"emulator installation contains {len(matched)} {model.path!r} models"
        )
    actual = file_sha256(matched[0])
    if actual != model.sha256:
        raise PreparationError(
            f"emulator model checksum is {actual}, expected {model.sha256}"
        )


def replace_link(link: Path, executable: Path) -> None:
    if link.is_symlink():
        link.unlink()
    elif link.exists():
        raise PreparationError(f"refusing to replace non-link path {link}")
    link.symlink_to(executable)


def run_command(command: tuple[str, ...], cwd: Path) -> None:
    try:
        subprocess.run(command, cwd=cwd, check=True)
    except (OSError, subprocess.SubprocessError) as error:
        raise PreparationError(f"command failed: {' '.join(command)}: {error}") from error
