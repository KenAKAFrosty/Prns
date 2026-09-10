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
        "validation/hygiene/embedded_assurance_selection.py",
        "validation/manifest.toml",
    }
)
RESOURCE_FILES = COMMON_FILES | frozenset(
    {
        ".cargo/config.toml",
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
    }
)
PILOT_FILES = COMMON_FILES | frozenset(
    {
        "tools/release/install-release-esp-toolchain.sh",
        "tools/release/release-esp-toolchain-identity.sh",
        "tools/release/verify-release-esp-toolchain.sh",
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
    "personal-hopspot/embedded/",
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
    "personal-hopspot/builder/",
    "personal-hopspot/embedded/",
    "personal-hopspot/memory/",
    "personal-rns/",
    "prns-core/",
    "prns-interfaces/impls/embassy/",
    "prns-runtime/core/",
    "prns-runtime/impls/embassy/",
)


class Lane(Enum):
    RESOURCES = "resources"
    MIRI = "miri"
    ISA = "isa"
    PILOTS = "pilots"


@dataclass(frozen=True)
class Selection:
    forced: bool
    resources: tuple[str, ...]
    miri: tuple[str, ...]
    isa: tuple[str, ...]
    pilots: tuple[str, ...]

    def paths(self, lane: Lane) -> tuple[str, ...]:
        match lane:
            case Lane.RESOURCES:
                return self.resources
            case Lane.MIRI:
                return self.miri
            case Lane.ISA:
                return self.isa
            case Lane.PILOTS:
                return self.pilots

    def required(self, lane: Lane) -> bool:
        return self.forced or bool(self.paths(lane))


class SelectionError(RuntimeError):
    pass


def selection_for_paths(paths: Iterable[str], *, forced: bool = False) -> Selection:
    paths = tuple(paths)
    return Selection(
        forced=forced,
        resources=matching(paths, RESOURCE_FILES, RESOURCE_TREES),
        miri=matching(paths, MIRI_FILES, MIRI_TREES),
        isa=matching(paths, ISA_FILES, ISA_TREES),
        pilots=matching(paths, PILOT_FILES, PILOT_TREES),
    )


def matching(
    paths: Iterable[str], files: frozenset[str], trees: tuple[str, ...]
) -> tuple[str, ...]:
    return tuple(
        sorted(
            path
            for path in paths
            if path in files or any(path.startswith(prefix) for prefix in trees)
        )
    )


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
                stream.write(f"{lane.value}_required={required}\n")
            stream.write(
                f"required={str(selection.required(Lane.RESOURCES)).lower()}\n"
            )
        for lane in Lane:
            required = str(selection.required(lane)).lower()
            print(f"EMBEDDED_ASSURANCE_SELECTION: lane={lane.value} required={required}")
            for path in selection.paths(lane):
                print(f"matched {lane.value} {path}")
        return 0
    except (KeyError, OSError, SelectionError, subprocess.SubprocessError) as error:
        print(f"EMBEDDED_ASSURANCE_SELECTION_ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
