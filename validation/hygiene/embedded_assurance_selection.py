from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Iterable, Mapping


ROOT = Path(__file__).resolve().parents[2]
ZERO_SHA = "0" * 40
SHA_PATTERN = re.compile(r"[0-9a-f]{40}")
COMMON_FILES = frozenset(
    {
        ".github/workflows/ci.yml",
        "Cargo.lock",
        "Cargo.toml",
        "rust-toolchain.toml",
        "tools/prns",
        "tools/tasks.toml",
        "tools/build/embedded-assurance.sh",
        "validation/run.py",
        "validation/hygiene/embedded_assurance_selection.py",
        "validation/manifest.toml",
    }
)
RESOURCE_FILES = COMMON_FILES | frozenset(
    {
        ".cargo/config.toml",
        "release/flash/boards.json",
        "VERSION",
        "tools/build/embedded-resources.sh",
        "tools/release/install-release-esp-toolchain.sh",
        "tools/release/release-esp-toolchain-identity.sh",
        "tools/release/verify-release-esp-toolchain.sh",
        "validation/platforms/embedded.sh",
        "validation/platforms/esp32-firmware-check.sh",
        "validation/platforms/no-std-esp-build.sh",
        "validation/hardening/embedded_architectures.py",
    }
)
MIRI_FILES = COMMON_FILES | frozenset(
    {
        "validation/hardening/embedded-miri.toml",
        "validation/hardening/embedded_failure.py",
        "validation/hardening/embedded_miri.py",
        "validation/hardening/miri.sh",
    }
)
ISA_FILES = COMMON_FILES | frozenset(
    {
        "tools/release/install-release-esp-toolchain.sh",
        "tools/release/release-esp-toolchain-identity.sh",
        "tools/release/verify-release-esp-toolchain.sh",
        "validation/hardening/embedded-isa.toml",
        "validation/hardening/embedded_architectures.py",
        "validation/hardening/embedded_execution.py",
        "validation/hardening/embedded_failure.py",
        "validation/hardening/embedded_host.py",
        "validation/hardening/embedded_readiness/contract.py",
        "validation/hardening/embedded_readiness/prepare.py",
    }
)
PILOT_FILES = COMMON_FILES | frozenset(
    {
        "validation/hardening/embedded-platform.toml",
        "validation/hardening/embedded_architectures.py",
        "validation/hardening/embedded_execution.py",
        "validation/hardening/embedded_failure.py",
        "validation/hardening/embedded_host.py",
        "validation/hardening/embedded_readiness/prepare.py",
    }
)
RESOURCE_TREES = (
    "personal-hopspot/assurance-kernel/",
    "personal-hopspot/builder/",
    "personal-hopspot/core/",
    "personal-hopspot/embedded/",
    "personal-hopspot/memory/",
    "personal-hopspot/resources/",
    "personal-rns/",
    "prns-core/",
    "prns-flash-manifest/",
    "prns-interfaces/impls/embassy/",
    "prns-macros/",
    "prns-runtime/core/",
    "prns-runtime/impls/embassy/",
)
MIRI_TREES = (
    "personal-hopspot/assurance/",
    "personal-hopspot/core/",
    "personal-rns/",
    "prns-core/",
    "prns-interfaces/impls/embassy/",
    "prns-runtime/core/",
    "prns-runtime/impls/embassy/",
)
ISA_TREES = (
    "personal-hopspot/assurance/",
    "personal-hopspot/assurance-kernel/",
    "personal-hopspot/builder/",
    "personal-hopspot/core/",
    "personal-hopspot/embedded/",
    "personal-hopspot/memory/",
    "personal-rns/",
    "prns-core/",
    "prns-interfaces/impls/embassy/",
    "prns-runtime/core/",
    "prns-runtime/impls/embassy/",
    "validation/hardening/embedded_isa/",
)
PILOT_TREES = (
    "personal-hopspot/assurance/",
    "personal-hopspot/memory/",
    "validation/hardening/embedded_platform/",
)


class Lane(Enum):
    RESOURCES = "resources"
    MIRI = "miri"
    ISA = "isa"
    PILOTS = "pilots"


class Suite(Enum):
    EMBEDDED_BUILDS = "embedded-builds"
    ESP32_FIRMWARE = "esp32-firmware-check"
    EMBEDDED_MIRI_QUICK = "embedded-miri-quick"
    ISA_THUMBV7EM = "embedded-isa-thumbv7em"
    ISA_RISCV32IMAC = "embedded-isa-riscv32imac"
    ISA_XTENSA_ESP32S3 = "embedded-isa-xtensa-esp32s3"
    PILOT_NRF52840 = "embedded-platform-nrf52840"
    PILOT_ESP32S3 = "embedded-platform-esp32s3"


RESOURCE_SUITES = (Suite.EMBEDDED_BUILDS, Suite.ESP32_FIRMWARE)
MIRI_SUITES = (Suite.EMBEDDED_MIRI_QUICK,)
ISA_SUITES = (
    Suite.ISA_THUMBV7EM,
    Suite.ISA_RISCV32IMAC,
    Suite.ISA_XTENSA_ESP32S3,
)
PILOT_SUITES = (Suite.PILOT_NRF52840, Suite.PILOT_ESP32S3)
LANE_SUITES = {
    Lane.RESOURCES: RESOURCE_SUITES,
    Lane.MIRI: MIRI_SUITES,
    Lane.ISA: ISA_SUITES,
    Lane.PILOTS: PILOT_SUITES,
}


@dataclass(frozen=True)
class SelectedSuite:
    suite: Suite
    matched_paths: tuple[str, ...]


@dataclass(frozen=True)
class Selection:
    forced: bool
    selected: tuple[SelectedSuite, ...]

    def suites(self, lane: Lane) -> tuple[SelectedSuite, ...]:
        allowed = frozenset(LANE_SUITES[lane])
        return tuple(item for item in self.selected if item.suite in allowed)

    def suite_ids(self, lane: Lane) -> tuple[str, ...]:
        return tuple(item.suite.value for item in self.suites(lane))

    def paths(self, lane: Lane) -> tuple[str, ...]:
        return tuple(
            sorted(
                {
                    path
                    for item in self.suites(lane)
                    for path in item.matched_paths
                }
            )
        )

    def required(self, lane: Lane) -> bool:
        return bool(self.suites(lane))

    def includes(self, suite: Suite) -> bool:
        return any(item.suite is suite for item in self.selected)

    def complete_required_evidence(self) -> bool:
        return all(
            self.suite_ids(lane) == tuple(suite.value for suite in LANE_SUITES[lane])
            for lane in (Lane.RESOURCES, Lane.MIRI, Lane.ISA)
        )


class SelectionError(RuntimeError):
    pass


def selection_for_paths(paths: Iterable[str], *, forced: bool = False) -> Selection:
    paths = tuple(sorted(set(paths)))
    reasons = {suite: [] for suite in Suite}
    for path in paths:
        if matches(path, RESOURCE_FILES, RESOURCE_TREES):
            add_reason(reasons, RESOURCE_SUITES, path)
        if matches(path, MIRI_FILES, MIRI_TREES):
            add_reason(reasons, MIRI_SUITES, path)
        add_reason(reasons, isa_suites(path), path)
        add_reason(reasons, pilot_suites(path), path)
    selected = tuple(
        SelectedSuite(suite, tuple(reasons[suite]))
        for suite in Suite
        if forced or reasons[suite]
    )
    return Selection(forced=forced, selected=selected)


def add_reason(
    reasons: dict[Suite, list[str]], suites: tuple[Suite, ...], path: str
) -> None:
    for suite in suites:
        reasons[suite].append(path)


def isa_suites(path: str) -> tuple[Suite, ...]:
    if path.startswith("personal-hopspot/assurance-kernel/src/bin/platform/"):
        return ()
    for prefix, suites in (
        ("personal-hopspot/assurance-kernel/src/bin/thumbv7em.rs", (Suite.ISA_THUMBV7EM,)),
        ("personal-hopspot/assurance-kernel/linker/thumbv7em/", (Suite.ISA_THUMBV7EM,)),
        ("personal-hopspot/assurance-kernel/src/bin/riscv32imac.rs", (Suite.ISA_RISCV32IMAC,)),
        ("personal-hopspot/assurance-kernel/linker/riscv32imac/", (Suite.ISA_RISCV32IMAC,)),
        (
            "personal-hopspot/assurance-kernel/src/bin/xtensa_esp32s3.rs",
            (Suite.ISA_XTENSA_ESP32S3,),
        ),
        ("validation/hardening/embedded_isa/architecture/thumbv7em.py", (Suite.ISA_THUMBV7EM,)),
        ("validation/hardening/embedded_isa/architecture/riscv32imac.py", (Suite.ISA_RISCV32IMAC,)),
        (
            "validation/hardening/embedded_isa/architecture/xtensa_esp32s3.py",
            (Suite.ISA_XTENSA_ESP32S3,),
        ),
        ("personal-hopspot/builder/src/architecture/thumbv7em.rs", (Suite.ISA_THUMBV7EM,)),
        ("personal-hopspot/builder/src/architecture/riscv32imac.rs", (Suite.ISA_RISCV32IMAC,)),
        ("personal-hopspot/builder/src/architecture/xtensa.rs", (Suite.ISA_XTENSA_ESP32S3,)),
        (
            "personal-hopspot/builder/src/architecture/linker/rust_lld.rs",
            (Suite.ISA_THUMBV7EM, Suite.ISA_RISCV32IMAC),
        ),
        ("personal-hopspot/builder/src/architecture/linker/gnu_ld.rs", (Suite.ISA_XTENSA_ESP32S3,)),
        ("personal-hopspot/builder/src/platform/nrf52840/", (Suite.ISA_THUMBV7EM,)),
        (
            "personal-hopspot/builder/src/platform/esp.rs",
            (Suite.ISA_RISCV32IMAC, Suite.ISA_XTENSA_ESP32S3),
        ),
        ("personal-hopspot/embedded/nrf52840/", (Suite.ISA_THUMBV7EM,)),
        ("personal-hopspot/embedded/esp32/platform-assurance/", ()),
        ("personal-hopspot/embedded/esp32/boards/xiao-esp32-c6/", (Suite.ISA_RISCV32IMAC,)),
        ("personal-hopspot/embedded/esp32/boards/", (Suite.ISA_XTENSA_ESP32S3,)),
        (
            "personal-hopspot/embedded/esp32/",
            (Suite.ISA_RISCV32IMAC, Suite.ISA_XTENSA_ESP32S3),
        ),
        ("personal-hopspot/memory/src/profiles/nrf52840/", (Suite.ISA_THUMBV7EM,)),
        (
            "personal-hopspot/memory/src/profiles/espressif/",
            (Suite.ISA_RISCV32IMAC, Suite.ISA_XTENSA_ESP32S3),
        ),
        ("tools/release/", (Suite.ISA_XTENSA_ESP32S3,)),
    ):
        if path.startswith(prefix):
            return suites
    return ISA_SUITES if matches(path, ISA_FILES, ISA_TREES) else ()


def pilot_suites(path: str) -> tuple[Suite, ...]:
    for prefix, suites in (
        ("validation/hardening/embedded_isa/architecture/thumbv7em.py", ()),
        ("validation/hardening/embedded_isa/architecture/riscv32imac.py", ()),
        (
            "validation/hardening/embedded_isa/architecture/xtensa_esp32s3.py",
            (Suite.PILOT_ESP32S3,),
        ),
        ("validation/hardening/embedded_platform/platform/nrf52840.py", (Suite.PILOT_NRF52840,)),
        ("validation/hardening/embedded_platform/platform/esp32s3.py", (Suite.PILOT_ESP32S3,)),
        ("personal-hopspot/embedded/nrf52840/", (Suite.PILOT_NRF52840,)),
        ("personal-hopspot/embedded/esp32/boards/xiao-esp32-c6/", ()),
        ("personal-hopspot/embedded/esp32/", (Suite.PILOT_ESP32S3,)),
        ("personal-hopspot/memory/src/profiles/nrf52840/", (Suite.PILOT_NRF52840,)),
        ("personal-hopspot/memory/src/profiles/espressif/", (Suite.PILOT_ESP32S3,)),
        ("personal-hopspot/assurance-kernel/", (Suite.PILOT_NRF52840,)),
        ("personal-hopspot/builder/", (Suite.PILOT_ESP32S3,)),
        ("prns-flash-manifest/", (Suite.PILOT_ESP32S3,)),
        ("release/flash/", (Suite.PILOT_ESP32S3,)),
        ("tools/release/", (Suite.PILOT_ESP32S3,)),
        ("validation/hardening/embedded_isa/", (Suite.PILOT_ESP32S3,)),
    ):
        if path.startswith(prefix):
            return suites
    if path == "validation/hardening/embedded-isa.toml":
        return (Suite.PILOT_ESP32S3,)
    return PILOT_SUITES if matches(path, PILOT_FILES, PILOT_TREES) else ()


def matches(path: str, files: frozenset[str], trees: tuple[str, ...]) -> bool:
    return path in files or any(path.startswith(prefix) for prefix in trees)


def changed_paths(base: str, head: str) -> tuple[str, ...]:
    validate_sha(head, "head")
    validate_sha(base, "base")
    command = (
        ("git", "ls-tree", "-r", "--name-only", head)
        if base == ZERO_SHA
        else ("git", "diff", "--name-only", base, head)
    )
    result = subprocess.run(
        command,
        cwd=ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    )
    return tuple(path for path in result.stdout.splitlines() if path)


def github_selection(environment: Mapping[str, str]) -> Selection:
    event_name = environment.get("GITHUB_EVENT_NAME")
    if event_name == "workflow_dispatch":
        return selection_for_paths((), forced=True)
    head = environment.get("GITHUB_SHA", "")
    event_path = Path(environment.get("GITHUB_EVENT_PATH", ""))
    try:
        event = json.loads(event_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SelectionError(f"could not read GitHub event {event_path}: {error}") from error
    if event_name == "pull_request":
        base = event.get("pull_request", {}).get("base", {}).get("sha")
    elif event_name == "push":
        base = event.get("before")
    else:
        raise SelectionError(f"unsupported GitHub event {event_name!r}")
    if not isinstance(base, str):
        raise SelectionError(f"GitHub event {event_name!r} has no base commit")
    return selection_for_paths(changed_paths(base, head))


def validate_sha(value: str, name: str) -> None:
    if not SHA_PATTERN.fullmatch(value):
        raise SelectionError(f"{name} commit must be a lowercase full SHA")


def main() -> int:
    try:
        selection = github_selection(os.environ)
        output = Path(os.environ["GITHUB_OUTPUT"])
        with output.open("a", encoding="utf-8") as stream:
            for lane in Lane:
                required = str(selection.required(lane)).lower()
                suites = " ".join(selection.suite_ids(lane))
                stream.write(f"{lane.value}_required={required}\n")
                stream.write(f"{lane.value}_suites={suites}\n")
            for suite in RESOURCE_SUITES:
                output_name = suite.value.replace("-", "_")
                stream.write(
                    f"{output_name}_required="
                    f"{str(selection.includes(suite)).lower()}\n"
                )
            stream.write(
                "aggregate_required="
                f"{str(selection.complete_required_evidence()).lower()}\n"
            )
            stream.write(f"required={str(selection.required(Lane.RESOURCES)).lower()}\n")
        for lane in Lane:
            suites = selection.suites(lane)
            required = str(bool(suites)).lower()
            print(
                f"EMBEDDED_ASSURANCE_SELECTION: lane={lane.value} "
                f"required={required} suites={len(suites)}"
            )
            for selected in suites:
                print(f"selected {lane.value} {selected.suite.value}")
                for path in selected.matched_paths:
                    print(f"matched {selected.suite.value} {path}")
        return 0
    except (KeyError, OSError, SelectionError, subprocess.SubprocessError) as error:
        print(f"EMBEDDED_ASSURANCE_SELECTION_ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
