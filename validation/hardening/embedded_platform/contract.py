from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import TypeAlias

from validation.hardening.embedded_architectures import ARCHITECTURES
from validation.hardening.embedded_host import HostPlatform


ROOT = Path(__file__).resolve().parents[3]
INVENTORY_PATH = ROOT / "validation" / "hardening" / "embedded-platform.toml"


class InventoryError(RuntimeError):
    pass


class EmulatorKind(Enum):
    QEMU = "qemu"
    RENODE = "renode"


class ExecutionKind(Enum):
    ARCHITECTURE_QEMU = "architecture-qemu"
    RENODE = "renode"


class Milestone(Enum):
    APPLICATION_ENTRY = "application-entry"
    RUNTIME_INITIALIZED = "runtime-initialized"


@dataclass(frozen=True)
class BuildRecipe:
    workspace: Path
    manifest: Path
    package: str
    binary: str
    features: tuple[str, ...]


@dataclass(frozen=True)
class EmulatorPackage:
    host: HostPlatform
    source_url: str
    source_sha256: str


@dataclass(frozen=True)
class RenodeEmulator:
    executable: str
    identity: tuple[str, ...]
    source_repository: str
    source_revision: str
    packages: tuple[EmulatorPackage, ...]

    def package_for_host(self, host: HostPlatform) -> EmulatorPackage | None:
        return next((package for package in self.packages if package.host is host), None)


@dataclass(frozen=True)
class RenodeExecution:
    milestone_symbol: str
    emulator: RenodeEmulator
    platform_description: str
    platform_description_sha256: str


@dataclass(frozen=True)
class ArchitectureQemuExecution:
    machine: str
    cpu: str
    board: str


Execution: TypeAlias = RenodeExecution | ArchitectureQemuExecution


@dataclass(frozen=True)
class Platform:
    identifier: str
    suite: str
    architecture: str
    memory_profile: str
    build: BuildRecipe
    runner: str
    scenario: str
    milestone: Milestone
    execution: Execution
    timeout_seconds: int
    sources: tuple[Path, ...]

    @property
    def rust_target(self) -> str:
        return ARCHITECTURES[self.architecture]

    @property
    def emulator_kind(self) -> EmulatorKind:
        if isinstance(self.execution, RenodeExecution):
            return EmulatorKind.RENODE
        if isinstance(self.execution, ArchitectureQemuExecution):
            return EmulatorKind.QEMU
        raise TypeError(f"unknown platform execution {type(self.execution).__name__}")


@dataclass(frozen=True)
class Inventory:
    rust_toolchain: str
    platforms: tuple[Platform, ...]

    def platform_for_suite(self, suite: str) -> Platform:
        matches = tuple(platform for platform in self.platforms if platform.suite == suite)
        if len(matches) != 1:
            raise InventoryError(
                f"embedded platform suite {suite!r} resolves {len(matches)} platforms"
            )
        return matches[0]


def load_inventory(path: Path = INVENTORY_PATH) -> Inventory:
    from validation.hardening.embedded_platform.inventory import load

    return load(path)
