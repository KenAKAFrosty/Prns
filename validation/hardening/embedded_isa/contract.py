from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import TypeAlias

from validation.hardening.embedded_architectures import ARCHITECTURES


ROOT = Path(__file__).resolve().parents[3]
INVENTORY_PATH = ROOT / "validation" / "hardening" / "embedded-isa.toml"
IDENTIFIER = re.compile(r"[a-z0-9](?:[a-z0-9-]{0,78}[a-z0-9])?")
SHA256 = re.compile(r"[0-9a-f]{64}")
SEMVER = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
QEMU_IDENTITY = re.compile(
    r"QEMU emulator version (?P<version>[0-9]+\.[0-9]+\.[0-9]+)(?: \([^\r\n)]+\))?"
)


class InventoryError(RuntimeError):
    pass


class Compiler(Enum):
    UPSTREAM = "upstream"
    ESP = "esp"


class HostPlatform(Enum):
    LINUX_AMD64 = "linux-amd64"
    LINUX_ARM64 = "linux-arm64"
    MACOS_AMD64 = "macos-amd64"
    MACOS_ARM64 = "macos-arm64"
    WINDOWS_AMD64 = "windows-amd64"


@dataclass(frozen=True)
class Kernel:
    manifest: Path
    package: str
    host_feature: str
    host_binary: str
    scenario: str
    completed_scenarios: int
    sources: tuple[Path, ...]


@dataclass(frozen=True)
class QemuIdentity:
    banner: str
    version: str


@dataclass(frozen=True)
class SourceArchive:
    source_url: str
    source_sha256: str


@dataclass(frozen=True)
class EmulatorPackage:
    host: HostPlatform
    source_url: str
    source_sha256: str


@dataclass(frozen=True)
class HostedPackages:
    packages: tuple[EmulatorPackage, ...]

    def for_host(self, host: HostPlatform) -> EmulatorPackage:
        return next(package for package in self.packages if package.host is host)


EmulatorAcquisition: TypeAlias = SourceArchive | HostedPackages


@dataclass(frozen=True)
class Emulator:
    executable: str
    identity: QemuIdentity
    acquisition: EmulatorAcquisition


@dataclass(frozen=True)
class Architecture:
    identifier: str
    suite: str
    rust_target: str
    compiler: Compiler
    feature: str
    binary: str
    runner: str
    emulator: Emulator
    timeout_seconds: int


@dataclass(frozen=True)
class Inventory:
    kernel: Kernel
    rust_toolchain: str
    architectures: tuple[Architecture, ...]

    def architecture_for_suite(self, suite: str) -> Architecture:
        matches = tuple(
            architecture
            for architecture in self.architectures
            if architecture.suite == suite
        )
        if len(matches) != 1:
            raise InventoryError(
                f"embedded ISA suite {suite!r} resolves {len(matches)} architectures"
            )
        return matches[0]


def load_inventory(path: Path = INVENTORY_PATH) -> Inventory:
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise InventoryError(f"cannot load {relative(path)}: {error}") from error
    if document.get("schema") != 2:
        raise InventoryError("embedded ISA inventory schema must be 2")
    kernel = parse_kernel(document.get("kernel"))
    toolchain = table(document.get("toolchain"), "toolchain")
    rust_toolchain = exact_version(toolchain.get("rust"), "Rust toolchain")
    architectures = tuple(
        parse_architecture(entry) for entry in document.get("architecture", [])
    )
    if not architectures:
        raise InventoryError("embedded ISA inventory is empty")
    for attribute in ("identifier", "suite", "rust_target", "runner"):
        values = [getattr(architecture, attribute) for architecture in architectures]
        if len(values) != len(set(values)):
            raise InventoryError(f"embedded ISA inventory repeats {attribute}")
    return Inventory(
        kernel=kernel,
        rust_toolchain=rust_toolchain,
        architectures=architectures,
    )


def parse_kernel(value: object) -> Kernel:
    entry = table(value, "kernel")
    completed = positive_integer(entry.get("completed_scenarios"), "completed scenarios")
    return Kernel(
        manifest=repository_file(entry.get("manifest"), "kernel manifest"),
        package=identifier(entry.get("package"), "kernel package"),
        host_feature=identifier(entry.get("host_feature"), "host feature"),
        host_binary=identifier(entry.get("host_binary"), "host binary"),
        scenario=identifier(entry.get("scenario"), "kernel scenario"),
        completed_scenarios=completed,
        sources=repository_paths(entry.get("sources"), "kernel sources"),
    )


def parse_architecture(value: object) -> Architecture:
    entry = table(value, "architecture")
    emulator = Emulator(
        executable=identifier(entry.get("emulator"), "emulator executable"),
        identity=qemu_identity(entry.get("emulator_identity")),
        acquisition=emulator_acquisition(entry),
    )
    architecture = identifier(entry.get("id"), "architecture")
    try:
        rust_target = ARCHITECTURES[architecture]
    except KeyError as error:
        raise InventoryError(
            f"unknown embedded ISA architecture {architecture!r}"
        ) from error
    return Architecture(
        identifier=architecture,
        suite=identifier(entry.get("suite"), "suite"),
        rust_target=rust_target,
        compiler=compiler(entry.get("compiler")),
        feature=identifier(entry.get("feature"), "architecture feature"),
        binary=identifier(entry.get("binary"), "architecture binary"),
        runner=f"qemu-{architecture}",
        emulator=emulator,
        timeout_seconds=positive_integer(entry.get("timeout_seconds"), "timeout"),
    )


def compiler(value: object) -> Compiler:
    try:
        return Compiler(value)
    except (TypeError, ValueError) as error:
        raise InventoryError(f"invalid embedded ISA compiler {value!r}") from error


def table(value: object, name: str) -> dict:
    if not isinstance(value, dict):
        raise InventoryError(f"embedded ISA {name} must be a table")
    return value


def identifier(value: object, name: str) -> str:
    if not isinstance(value, str) or IDENTIFIER.fullmatch(value) is None:
        raise InventoryError(f"invalid embedded ISA {name} identifier {value!r}")
    return value


def exact_version(value: object, name: str) -> str:
    if not isinstance(value, str) or SEMVER.fullmatch(value) is None:
        raise InventoryError(f"embedded ISA {name} is not exact-pinned")
    return value


def qemu_identity(value: object) -> QemuIdentity:
    if not isinstance(value, str):
        raise InventoryError("embedded ISA emulator identity must be a string")
    matched = QEMU_IDENTITY.fullmatch(value)
    if matched is None:
        raise InventoryError("embedded ISA emulator identity must be an exact QEMU banner")
    return QemuIdentity(banner=value, version=matched.group("version"))


def emulator_acquisition(entry: dict) -> EmulatorAcquisition:
    source_url = entry.get("emulator_source_url")
    source_sha256 = entry.get("emulator_source_sha256")
    packages = entry.get("emulator_package")
    has_source = source_url is not None or source_sha256 is not None
    has_packages = packages is not None
    if has_source == has_packages:
        raise InventoryError(
            "embedded ISA emulator needs exactly one source archive or package set"
        )
    if has_source:
        return SourceArchive(
            source_url=https_url(source_url),
            source_sha256=sha256(source_sha256),
        )
    if not isinstance(packages, list) or not packages:
        raise InventoryError("embedded ISA emulator package set is empty")
    parsed = tuple(emulator_package(package) for package in packages)
    hosts = tuple(package.host for package in parsed)
    if len(hosts) != len(set(hosts)):
        raise InventoryError("embedded ISA emulator package set repeats a host")
    missing_hosts = set(HostPlatform).difference(hosts)
    if missing_hosts:
        missing = ", ".join(sorted(host.value for host in missing_hosts))
        raise InventoryError(f"embedded ISA emulator package set is missing {missing}")
    return HostedPackages(parsed)


def emulator_package(value: object) -> EmulatorPackage:
    entry = table(value, "emulator package")
    try:
        host = HostPlatform(entry.get("host"))
    except (TypeError, ValueError) as error:
        raise InventoryError(
            f"invalid embedded ISA emulator package host {entry.get('host')!r}"
        ) from error
    return EmulatorPackage(
        host=host,
        source_url=https_url(entry.get("source_url")),
        source_sha256=sha256(entry.get("source_sha256")),
    )


def sha256(value: object) -> str:
    if not isinstance(value, str) or SHA256.fullmatch(value) is None:
        raise InventoryError("embedded ISA emulator source checksum must be lowercase SHA-256")
    return value


def https_url(value: object) -> str:
    if not isinstance(value, str) or not value.startswith("https://"):
        raise InventoryError("embedded ISA emulator source URL must use HTTPS")
    return value


def positive_integer(value: object, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise InventoryError(f"embedded ISA {name} must be a positive integer")
    return value


def repository_paths(value: object, name: str) -> tuple[Path, ...]:
    if (
        not isinstance(value, list)
        or not value
        or any(not isinstance(item, str) or not item for item in value)
        or len(value) != len(set(value))
    ):
        raise InventoryError(f"embedded ISA {name} must contain unique paths")
    return tuple(repository_path(item, name) for item in value)


def repository_path(value: object, name: str) -> Path:
    if not isinstance(value, str) or not value:
        raise InventoryError(f"embedded ISA {name} path is invalid")
    path = ROOT / value
    try:
        resolved = path.resolve(strict=True)
    except OSError as error:
        raise InventoryError(f"embedded ISA {name} path is missing: {value}") from error
    if ROOT not in resolved.parents:
        raise InventoryError(f"embedded ISA {name} path escapes the repository: {value}")
    return resolved


def repository_file(value: object, name: str) -> Path:
    path = repository_path(value, name)
    if not path.is_file():
        raise InventoryError(f"embedded ISA {name} is not a file: {relative(path)}")
    return path


def relative(path: Path) -> str:
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return path.as_posix()
