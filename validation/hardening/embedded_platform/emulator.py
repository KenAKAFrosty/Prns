from __future__ import annotations

import os
import shutil
from pathlib import Path

from validation.hardening.embedded_isa.contract import (
    Architecture,
    HostedPackages,
    SourceArchive,
)
from validation.hardening.embedded_platform import artifacts
from validation.hardening.embedded_platform.contract import (
    EmulatorKind,
    RenodeExecution,
)
from validation.hardening.embedded_platform.discovery import (
    RENODE_EXECUTABLE_ENV,
    RENODE_ROOT_ENV,
    description_candidates,
    file_sha256,
)
from validation.hardening.embedded_platform.error import EmbeddedPlatformError


DOCTOR = "./tools/prns doctor embedded-assurance"


def renode_executable(execution: RenodeExecution) -> Path:
    override = os.environ.get(RENODE_EXECUTABLE_ENV)
    discovered = override or shutil.which(execution.emulator.executable)
    if discovered is None:
        raise EmbeddedPlatformError(
            f"required emulator {execution.emulator.executable} is unavailable; run {DOCTOR}"
        )
    executable = Path(discovered).resolve(strict=True)
    if not executable.is_file():
        raise EmbeddedPlatformError(f"emulator is not a regular file: {executable}")
    return executable


def platform_description(
    execution: RenodeExecution, executable: Path
) -> tuple[Path, str]:
    relative = Path(execution.platform_description)
    inspected = []
    roots = description_candidates(executable, os.environ.get(RENODE_ROOT_ENV))
    for root in roots:
        candidate = root / relative
        try:
            resolved = candidate.resolve(strict=True)
        except OSError:
            continue
        if not resolved.is_file():
            continue
        actual = file_sha256(resolved)
        inspected.append((resolved, actual))
        if actual == execution.platform_description_sha256:
            return resolved, actual
    if inspected:
        found = ", ".join(f"{path}={digest}" for path, digest in inspected)
        raise EmbeddedPlatformError(
            "platform model checksum mismatch; expected "
            f"{execution.platform_description_sha256}, found {found}; run {DOCTOR}"
        )
    raise EmbeddedPlatformError(
        f"cannot locate {execution.platform_description} beside {executable}; "
        f"set {RENODE_ROOT_ENV} or run {DOCTOR}"
    )


def renode_evidence(
    execution: RenodeExecution,
    executable: Path,
    identity: str,
    description: Path,
    description_sha256: str,
    effective_description: Path,
    effective_description_sha256: str,
) -> artifacts.EmulatorEvidence:
    details = [
        f"emulator-source={execution.emulator.source_repository}",
        f"emulator-source-revision={execution.emulator.source_revision}",
        f"platform-description={description}",
        f"platform-description-sha256={description_sha256}",
        f"effective-platform-description={effective_description}",
        f"effective-platform-description-sha256={effective_description_sha256}",
    ]
    for package in execution.emulator.packages:
        details.extend(
            (
                f"emulator-package-{package.host.value}={package.source_url}",
                f"emulator-package-{package.host.value}-sha256={package.source_sha256}",
            )
        )
    return artifacts.EmulatorEvidence(
        kind=EmulatorKind.RENODE,
        identity=identity,
        executable_sha256=file_sha256(executable),
        details=tuple(details),
    )


def qemu_evidence(
    architecture: Architecture, executable: Path, identity: str
) -> artifacts.EmulatorEvidence:
    acquisition = architecture.emulator.acquisition
    match acquisition:
        case SourceArchive(source_url=url, source_sha256=checksum):
            details = (
                f"emulator-source={url}",
                f"emulator-source-sha256={checksum}",
            )
        case HostedPackages(packages=packages):
            details = tuple(
                value
                for package in packages
                for value in (
                    f"emulator-package-{package.host.value}={package.source_url}",
                    f"emulator-package-{package.host.value}-sha256={package.source_sha256}",
                )
            )
    return artifacts.EmulatorEvidence(
        kind=EmulatorKind.QEMU,
        identity=identity,
        executable_sha256=file_sha256(executable),
        details=details,
    )
