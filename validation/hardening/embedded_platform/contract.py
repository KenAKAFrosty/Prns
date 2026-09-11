from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass
from enum import Enum
from pathlib import Path

from validation.hardening.embedded_architectures import ARCHITECTURES
from validation.hardening.embedded_host import HostPlatform


ROOT = Path(__file__).resolve().parents[3]
INVENTORY_PATH = ROOT / "validation" / "hardening" / "embedded-platform.toml"
IDENTIFIER = re.compile(r"[a-z0-9](?:[a-z0-9-]{0,78}[a-z0-9])?")
SYMBOL = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
SHA256 = re.compile(r"[0-9a-f]{64}")
REVISION = re.compile(r"[0-9a-f]{40}")
SEMVER = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")


class InventoryError(RuntimeError):
    pass


class EmulatorKind(Enum):
    QEMU = "qemu"
    RENODE = "renode"


class Milestone(Enum):
    APPLICATION_ENTRY = "application-entry"
    RUNTIME_INITIALIZED = "runtime-initialized"


@dataclass(frozen=True)
class EmulatorPackage:
    host: HostPlatform
    source_url: str
    source_sha256: str


@dataclass(frozen=True)
class Emulator:
    kind: EmulatorKind
    executable: str
    identity: tuple[str, ...]
    source_repository: str
    source_revision: str
    packages: tuple[EmulatorPackage, ...]

    def package_for_host(self, host: HostPlatform) -> EmulatorPackage | None:
        return next((package for package in self.packages if package.host is host), None)


@dataclass(frozen=True)
class Platform:
    identifier: str
    suite: str
    architecture: str
    rust_target: str
    memory_profile: str
    feature: str
    binary: str
    runner: str
    scenario: str
    milestone: Milestone
    milestone_symbol: str
    emulator: Emulator
    platform_description: str
    platform_description_sha256: str
    timeout_seconds: int
    sources: tuple[Path, ...]


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
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise InventoryError(f"cannot load {relative(path)}: {error}") from error
    if document.get("schema") != 1:
        raise InventoryError("embedded platform inventory schema must be 1")
    toolchain = table(document.get("toolchain"), "toolchain")
    platforms = tuple(parse_platform(entry) for entry in document.get("platform", []))
    if not platforms:
        raise InventoryError("embedded platform inventory is empty")
    for attribute in ("identifier", "suite", "runner"):
        values = [getattr(platform, attribute) for platform in platforms]
        if len(values) != len(set(values)):
            raise InventoryError(f"embedded platform inventory repeats {attribute}")
    return Inventory(
        rust_toolchain=exact_version(toolchain.get("rust"), "Rust toolchain"),
        platforms=platforms,
    )


def parse_platform(value: object) -> Platform:
    entry = table(value, "platform")
    architecture = identifier(entry.get("architecture"), "architecture")
    rust_target = text(entry.get("rust_target"), "Rust target")
    expected_target = ARCHITECTURES.get(architecture)
    if rust_target != expected_target:
        raise InventoryError(
            f"embedded platform architecture {architecture!r} requires Rust target "
            f"{expected_target!r}, found {rust_target!r}"
        )
    packages = tuple(
        parse_emulator_package(package) for package in entry.get("emulator_package", [])
    )
    if not packages:
        raise InventoryError("embedded platform emulator package set is empty")
    hosts = tuple(package.host for package in packages)
    if len(hosts) != len(set(hosts)):
        raise InventoryError("embedded platform emulator package set repeats a host")
    emulator = Emulator(
        kind=enum_value(EmulatorKind, entry.get("emulator_kind"), "emulator kind"),
        executable=identifier(entry.get("emulator"), "emulator executable"),
        identity=nonempty_texts(entry.get("emulator_identity"), "emulator identity"),
        source_repository=https_url(entry.get("emulator_source_repository")),
        source_revision=revision(entry.get("emulator_source_revision")),
        packages=packages,
    )
    return Platform(
        identifier=identifier(entry.get("id"), "platform"),
        suite=identifier(entry.get("suite"), "suite"),
        architecture=architecture,
        rust_target=rust_target,
        memory_profile=identifier(entry.get("memory_profile"), "memory profile"),
        feature=identifier(entry.get("feature"), "feature"),
        binary=identifier(entry.get("binary"), "binary"),
        runner=identifier(entry.get("runner"), "runner"),
        scenario=identifier(entry.get("scenario"), "scenario"),
        milestone=enum_value(Milestone, entry.get("milestone"), "milestone"),
        milestone_symbol=symbol(entry.get("milestone_symbol")),
        emulator=emulator,
        platform_description=safe_relative_path(
            entry.get("platform_description"), "platform description"
        ),
        platform_description_sha256=sha256(entry.get("platform_description_sha256")),
        timeout_seconds=positive_integer(entry.get("timeout_seconds"), "timeout"),
        sources=repository_paths(entry.get("sources"), "platform sources"),
    )


def parse_emulator_package(value: object) -> EmulatorPackage:
    entry = table(value, "emulator package")
    return EmulatorPackage(
        host=enum_value(HostPlatform, entry.get("host"), "emulator package host"),
        source_url=https_url(entry.get("source_url")),
        source_sha256=sha256(entry.get("source_sha256")),
    )


def table(value: object, name: str) -> dict:
    if not isinstance(value, dict):
        raise InventoryError(f"embedded platform {name} must be a table")
    return value


def identifier(value: object, name: str) -> str:
    if not isinstance(value, str) or IDENTIFIER.fullmatch(value) is None:
        raise InventoryError(f"invalid embedded platform {name} identifier {value!r}")
    return value


def symbol(value: object) -> str:
    if not isinstance(value, str) or SYMBOL.fullmatch(value) is None:
        raise InventoryError(f"invalid embedded platform milestone symbol {value!r}")
    return value


def text(value: object, name: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise InventoryError(f"embedded platform {name} must be non-empty text")
    return value


def nonempty_texts(value: object, name: str) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise InventoryError(f"embedded platform {name} must be a list")
    values = tuple(text(item, name) for item in value)
    if not values or len(values) != len(set(values)):
        raise InventoryError(f"embedded platform {name} must be non-empty and unique")
    return values


def exact_version(value: object, name: str) -> str:
    if not isinstance(value, str) or SEMVER.fullmatch(value) is None:
        raise InventoryError(f"embedded platform {name} is not exact-pinned")
    return value


def enum_value(enum_type: type[Enum], value: object, name: str):
    try:
        return enum_type(value)
    except (TypeError, ValueError) as error:
        raise InventoryError(f"invalid embedded platform {name} {value!r}") from error


def revision(value: object) -> str:
    if not isinstance(value, str) or REVISION.fullmatch(value) is None:
        raise InventoryError("embedded platform source revision must be a full commit SHA")
    return value


def sha256(value: object) -> str:
    if not isinstance(value, str) or SHA256.fullmatch(value) is None:
        raise InventoryError("embedded platform checksum must be lowercase SHA-256")
    return value


def https_url(value: object) -> str:
    if not isinstance(value, str) or not value.startswith("https://"):
        raise InventoryError("embedded platform source URL must use HTTPS")
    return value


def safe_relative_path(value: object, name: str) -> str:
    path = Path(text(value, name))
    if path.is_absolute() or ".." in path.parts:
        raise InventoryError(f"embedded platform {name} must be a safe relative path")
    return path.as_posix()


def repository_paths(value: object, name: str) -> tuple[Path, ...]:
    if not isinstance(value, list) or not value:
        raise InventoryError(f"embedded platform {name} must be a non-empty list")
    paths = tuple(ROOT / safe_relative_path(item, name) for item in value)
    missing = tuple(path for path in paths if not path.exists())
    if missing:
        raise InventoryError(
            "embedded platform source paths do not exist: "
            + ", ".join(relative(path) for path in missing)
        )
    return paths


def positive_integer(value: object, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise InventoryError(f"embedded platform {name} must be a positive integer")
    return value


def relative(path: Path) -> str:
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return str(path)
