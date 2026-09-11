from __future__ import annotations

import subprocess
from pathlib import Path

from validation.hardening.embedded_isa.contract import (
    INVENTORY_PATH,
    ROOT,
    Architecture,
    Kernel,
)


IMPLEMENTATION_PATH = Path(__file__).resolve().parent


def record(
    architecture: Architecture,
    kernel: Kernel,
    emulator_executable: Path,
    target_executable: Path,
    transcript: Path,
    log: Path,
    cargo_version: str,
    rustc_version: str,
    qemu_version: str,
    extra_sources: tuple[Path, ...] = (),
) -> None:
    output = transcript.parent / f"{architecture.identifier}.assurance.json"
    command = [
        str(ROOT / "tools" / "prns"),
        "build",
        "embedded",
        "assurance",
        "record",
        "isa",
        "--architecture",
        architecture.identifier,
        "--scenario",
        kernel.scenario,
        "--runner",
        architecture.runner,
        "--completed-scenarios",
        str(kernel.completed_scenarios),
        "--cargo-version",
        cargo_version,
        "--rustc-version",
        rustc_version,
        "--qemu-version",
        qemu_version,
        "--qemu-executable",
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
    for source in (
        INVENTORY_PATH,
        IMPLEMENTATION_PATH,
        *kernel.sources,
        *extra_sources,
    ):
        command.extend(("--source", str(source)))
    subprocess.run(command, cwd=ROOT, check=True)
