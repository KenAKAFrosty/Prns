from __future__ import annotations

import os
import shutil
from dataclasses import dataclass
from pathlib import Path

from validation.hardening.embedded_isa.architecture import target_build_for
from validation.hardening.embedded_isa.contract import Architecture, Compiler
from validation.hardening.embedded_isa.error import EmbeddedIsaError
from validation.hardening.embedded_isa.process import tool_version
from validation.hardening.embedded_readiness import contract as readiness_contract
from validation.hardening.embedded_readiness.error import ReadinessError


DOCTOR = "./tools/prns doctor embedded-assurance"


@dataclass(frozen=True)
class TargetToolchain:
    channel: str
    cargo_version: str
    rustc_version: str
    linker_identity: str
    cargo_arguments: tuple[str, ...]
    search_paths: tuple[Path, ...]
    proof_sources: tuple[Path, ...]


def resolve(
    architecture: Architecture,
    upstream_channel: str,
    upstream_cargo_version: str,
    upstream_rustc_version: str,
) -> TargetToolchain:
    match architecture.compiler:
        case Compiler.UPSTREAM:
            return TargetToolchain(
                channel=upstream_channel,
                cargo_version=upstream_cargo_version,
                rustc_version=upstream_rustc_version,
                linker_identity=f"rust-lld bundled with {upstream_rustc_version}",
                cargo_arguments=(),
                search_paths=(),
                proof_sources=(),
            )
        case Compiler.ESP:
            return esp(architecture)


def esp(architecture: Architecture) -> TargetToolchain:
    try:
        identity = readiness_contract.load_esp_identity()
        environment = readiness_contract.load_esp_environment(
            readiness_contract.home_directory()
        )
    except ReadinessError as error:
        raise EmbeddedIsaError(f"{error}; run {DOCTOR}") from error
    build = target_build_for(architecture.identifier)
    linker = executable(build.linker, environment.search_paths)
    cargo_version = tool_version(("cargo", "+esp", "--version"), "cargo ", DOCTOR)
    rustc_version = tool_version(("rustc", "+esp", "--version"), "rustc ", DOCTOR)
    if rustc_version != identity.rustc_banner:
        raise EmbeddedIsaError(
            f"ESP Rust identity is {rustc_version!r}, expected {identity.rustc_banner!r}; "
            f"run {DOCTOR}"
        )
    linker_version = tool_version((str(linker), "--version"), identity.gcc_banner, DOCTOR)
    if linker_version != identity.gcc_banner:
        raise EmbeddedIsaError(
            f"ESP linker identity is {linker_version!r}, expected {identity.gcc_banner!r}; "
            f"run {DOCTOR}"
        )
    return TargetToolchain(
        channel="esp",
        cargo_version=cargo_version,
        rustc_version=rustc_version,
        linker_identity=linker_version,
        cargo_arguments=build.cargo_arguments(linker, architecture.rust_target),
        search_paths=environment.search_paths,
        proof_sources=(
            readiness_contract.ESP_IDENTITY_PATH,
            Path(readiness_contract.__file__).resolve(strict=True),
        ),
    )


def executable(command: str, search_paths: tuple[Path, ...]) -> Path:
    discovered = shutil.which(
        command,
        path=os.pathsep.join(str(path) for path in search_paths),
    )
    if discovered is None:
        raise EmbeddedIsaError(f"required tool {command} is unavailable; run {DOCTOR}")
    path = Path(discovered).resolve(strict=True)
    if not path.is_file():
        raise EmbeddedIsaError(f"tool is not a regular file: {path}")
    return path
