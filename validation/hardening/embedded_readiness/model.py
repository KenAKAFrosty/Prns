from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path

from validation.hardening.embedded_isa.contract import Architecture
from validation.hardening.embedded_platform.contract import Platform


class CheckState(Enum):
    READY = "ready"
    MISSING = "missing"
    MISMATCH = "version-mismatch"


class ReadinessLane(Enum):
    RESOURCES = "resources"
    MIRI = "miri"
    ISA = "isa"
    PILOT = "pilot"


class ReadinessStatus(Enum):
    READY = "ready"
    NOT_READY = "not-ready"


@dataclass(frozen=True)
class CommandOutput:
    returncode: int
    stdout: str
    stderr: str

    def lines(self) -> tuple[str, ...]:
        return tuple(
            line.strip()
            for line in f"{self.stdout}\n{self.stderr}".splitlines()
            if line.strip()
        )


@dataclass(frozen=True)
class ReadinessCheck:
    lane: ReadinessLane
    subject: str
    state: CheckState
    detail: str
    setup: tuple[str, ...] = ()

    def passed(self) -> bool:
        return self.state is CheckState.READY


@dataclass(frozen=True)
class EspIdentity:
    espup_version: str
    rust_toolchain_version: str
    rustc_banner: str
    crosstool_version: str
    gcc_banner: str
    objdump_banner: str


@dataclass(frozen=True)
class EspEnvironment:
    search_paths: tuple[Path, ...]
    libclang_path: Path | None


@dataclass(frozen=True)
class ReadinessContract:
    isa_toolchain: str
    architectures: tuple[Architecture, ...]
    platforms: tuple[Platform, ...]
    miri_toolchain: str
    miri_scenarios: int
    esp_identity: EspIdentity
    esp_environment: EspEnvironment
