from __future__ import annotations

import subprocess
from pathlib import Path

from validation.hardening.embedded_platform.contract import (
    INVENTORY_PATH,
    ROOT,
    Platform,
)


IMPLEMENTATION_PATH = Path(__file__).resolve().parent


def record(
    platform: Platform,
    emulator_executable: Path,
    target_executable: Path,
    transcript: Path,
    log: Path,
    cargo_version: str,
    rustc_version: str,
    linker_identity: str,
    emulator_identity: str,
) -> None:
    output = transcript.parent / f"{platform.identifier}.assurance.json"
    command = [
        str(ROOT / "tools" / "prns"),
        "build",
        "embedded",
        "assurance",
        "record",
        "platform",
        "--platform",
        platform.identifier,
        "--scenario",
        platform.scenario,
        "--runner",
        platform.runner,
        "--milestone",
        platform.milestone.value,
        "--cargo-version",
        cargo_version,
        "--rustc-version",
        rustc_version,
        "--linker-version",
        linker_identity,
        "--emulator",
        platform.emulator.kind.value,
        "--emulator-version",
        emulator_identity,
        "--emulator-executable",
        str(emulator_executable),
        "--transcript",
        str(transcript),
        "--executable",
        str(target_executable),
        "--log",
        str(log),
        "--output",
        str(output),
    ]
    for source in (INVENTORY_PATH, IMPLEMENTATION_PATH, *platform.sources):
        command.extend(("--source", str(source)))
    subprocess.run(command, cwd=ROOT, check=True)
