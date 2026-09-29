from __future__ import annotations

import os
import re
import shlex
from pathlib import Path

from validation.hardening import embedded_miri
from validation.hardening.embedded_isa.contract import load_inventory as load_isa_inventory
from validation.hardening.embedded_platform.contract import (
    load_inventory as load_platform_inventory,
)
from validation.hardening.embedded_readiness.error import ReadinessError
from validation.hardening.embedded_readiness.model import (
    EspEnvironment,
    EspIdentity,
    ReadinessContract,
)


ROOT = Path(__file__).resolve().parents[3]
ESP_IDENTITY_PATH = ROOT / "tools" / "release" / "release-esp-toolchain-identity.sh"
ASSIGNMENT = re.compile(r"[A-Z][A-Z0-9_]*")


def load_contract(home: Path | None = None) -> ReadinessContract:
    isa = load_isa_inventory()
    platform = load_platform_inventory()
    if platform.rust_toolchain != isa.rust_toolchain:
        raise ReadinessError(
            "embedded ISA and platform inventories require different Rust toolchains"
        )
    scenarios = embedded_miri.load_inventory()
    resolved_home = home if home is not None else home_directory()
    return ReadinessContract(
        isa_toolchain=isa.rust_toolchain,
        architectures=isa.architectures,
        platforms=platform.platforms,
        miri_toolchain=embedded_miri.nightly_toolchain(),
        miri_scenarios=len(scenarios),
        esp_identity=load_esp_identity(),
        esp_environment=load_esp_environment(resolved_home),
    )


def load_esp_identity(path: Path = ESP_IDENTITY_PATH) -> EspIdentity:
    values = assignments(path.read_text(encoding="utf-8"))
    required = (
        "ESPUP_VERSION",
        "ESP_RUST_TOOLCHAIN_VERSION",
        "ESP_RUSTC_BANNER",
        "ESP_CROSSTOOL_VERSION",
        "ESP_GCC_BANNER",
        "ESP_OBJDUMP_BANNER",
    )
    missing = tuple(name for name in required if name not in values)
    if missing:
        raise ReadinessError(f"ESP identity is missing {', '.join(missing)}")
    return EspIdentity(
        espup_version=values["ESPUP_VERSION"],
        rust_toolchain_version=values["ESP_RUST_TOOLCHAIN_VERSION"],
        rustc_banner=values["ESP_RUSTC_BANNER"],
        crosstool_version=values["ESP_CROSSTOOL_VERSION"],
        gcc_banner=values["ESP_GCC_BANNER"],
        objdump_banner=values["ESP_OBJDUMP_BANNER"],
    )


def assignments(contents: str) -> dict[str, str]:
    values = {}
    for source in contents.splitlines():
        line = source.strip()
        if line.startswith("export "):
            line = line.removeprefix("export ").lstrip()
        name, separator, raw = line.partition("=")
        if not separator or ASSIGNMENT.fullmatch(name) is None:
            continue
        try:
            parsed = shlex.split(raw, posix=True)
        except ValueError as error:
            raise ReadinessError(f"invalid shell assignment for {name}: {error}") from error
        if len(parsed) != 1:
            raise ReadinessError(f"invalid shell assignment for {name}")
        values[name] = parsed[0]
    return values


def home_directory() -> Path:
    value = os.environ.get("HOME") or os.environ.get("USERPROFILE")
    if value is None:
        raise ReadinessError("cannot resolve the developer home directory")
    return Path(value)


def load_esp_environment(home: Path) -> EspEnvironment:
    paths = []
    inherited_libclang = os.environ.get("LIBCLANG_PATH")
    libclang_path = Path(inherited_libclang) if inherited_libclang else None
    export_path = home / "export-esp.sh"
    if export_path.is_file():
        values = assignments(export_path.read_text(encoding="utf-8"))
        for value in values.get("PATH", "").split(":"):
            if value and value not in {"$PATH", "${PATH}"}:
                paths.append(expand_home(value, home))
        if value := values.get("LIBCLANG_PATH"):
            libclang_path = expand_home(value, home)
    root = home / ".rustup" / "toolchains" / "esp" / "xtensa-esp-elf"
    paths.append(root / "bin")
    if root.is_dir():
        for release in sorted(root.iterdir()):
            paths.append(release / "xtensa-esp-elf" / "bin")
    inherited = os.environ.get("PATH", "")
    paths.extend(Path(value) for value in inherited.split(os.pathsep) if value)
    return EspEnvironment(tuple(unique_paths(paths)), libclang_path)


def expand_home(value: str, home: Path) -> Path:
    expanded = value.replace("${HOME}", str(home)).replace("$HOME", str(home))
    if expanded.startswith("~/"):
        return home / expanded[2:]
    return Path(expanded)


def unique_paths(paths: list[Path]) -> list[Path]:
    seen = set()
    result = []
    for path in paths:
        key = str(path)
        if key not in seen:
            seen.add(key)
            result.append(path)
    return result
