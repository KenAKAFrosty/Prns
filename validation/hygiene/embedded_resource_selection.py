from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Iterable, Mapping


ROOT = Path(__file__).resolve().parents[2]
ZERO_SHA = "0" * 40
SHA_PATTERN = re.compile(r"[0-9a-f]{40}")
RESOURCE_FILES = frozenset(
    {
        ".cargo/config.toml",
        ".github/workflows/ci.yml",
        "Cargo.lock",
        "Cargo.toml",
        "VERSION",
        "rust-toolchain.toml",
        "tools/prns",
        "tools/build/embedded-resources.sh",
        "tools/tasks.toml",
        "tools/release/install-release-esp-toolchain.sh",
        "tools/release/release-esp-toolchain-identity.sh",
        "tools/release/verify-release-esp-toolchain.sh",
        "validation/hygiene/embedded_resource_selection.py",
        "validation/manifest.toml",
        "validation/platforms/embedded.sh",
        "validation/platforms/esp32-firmware-check.sh",
        "validation/platforms/no-std-esp-build.sh",
    }
)
RESOURCE_TREES = (
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


class SelectionError(RuntimeError):
    pass


def affected_paths(paths: Iterable[str]) -> tuple[str, ...]:
    return tuple(
        sorted(
            path
            for path in paths
            if path in RESOURCE_FILES
            or any(path.startswith(prefix) for prefix in RESOURCE_TREES)
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


def github_selection(environment: Mapping[str, str]) -> tuple[bool, tuple[str, ...]]:
    event_name = environment.get("GITHUB_EVENT_NAME")
    if event_name == "workflow_dispatch":
        return True, ()
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
    matches = affected_paths(changed_paths(base, head))
    return bool(matches), matches


def validate_sha(value: str, name: str) -> None:
    if not SHA_PATTERN.fullmatch(value):
        raise SelectionError(f"{name} commit must be a lowercase full SHA")


def main() -> int:
    try:
        required, matches = github_selection(os.environ)
        output = Path(os.environ["GITHUB_OUTPUT"])
        with output.open("a", encoding="utf-8") as stream:
            stream.write(f"required={'true' if required else 'false'}\n")
        print(f"EMBEDDED_RESOURCE_SELECTION: required={str(required).lower()}")
        for path in matches:
            print(f"matched {path}")
        return 0
    except (KeyError, OSError, SelectionError, subprocess.SubprocessError) as error:
        print(f"EMBEDDED_RESOURCE_SELECTION_ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
