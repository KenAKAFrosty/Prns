#!/usr/bin/env python3
"""Fail closed unless a signed candidate has complete physical acceptance evidence."""

from __future__ import annotations

import argparse
from collections import defaultdict
from datetime import date
import hashlib
import json
from pathlib import Path
import sys


BOARDS = {"heltec-v4", "t-beam-supreme", "xiao-esp32-c6", "t-echo"}
SURFACES = {"web", "cli"}
OS_NAMES = {"macos", "windows", "linux"}
ARCHITECTURES = {
    ("macos", "aarch64"),
    ("macos", "x86_64"),
    ("linux", "x86_64"),
    ("linux", "aarch64"),
    ("windows", "x86_64"),
}
CLI_TARGETS = {
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
}
COMMON_SCENARIOS = {
    "fresh-install",
    "update",
    "correct-board",
    "incorrect-board",
    "zero-devices",
    "one-device",
    "multiple-devices",
    "boot-reset-recovery",
    "disconnect-before-write",
    "disconnect-during-write",
    "disconnect-before-reset",
    "corrupt-artifact",
    "signature-rejection",
    "verification-failure",
    "reset-failure",
    "post-flash-boot",
}
PROVISIONING_SCENARIOS = {"preserve", "configure", "clear"}
TECHO_SCENARIOS = {"missing-mount", "failed-copy", "failed-sync", "reboot-detection"}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def nonempty(record: dict, *fields: str) -> bool:
    return all(isinstance(record.get(field), str) and record[field].strip() for field in fields)


def validate(arguments: argparse.Namespace) -> list[str]:
    errors: list[str] = []
    acceptance = json.loads(arguments.acceptance.read_text(encoding="utf-8"))
    manifest = json.loads(arguments.manifest.read_text(encoding="utf-8"))
    release = manifest.get("release", {})
    candidate = acceptance.get("candidate", {})
    if acceptance.get("schema") != 1:
        errors.append("acceptance schema must be 1")
    if candidate.get("version") != release.get("version"):
        errors.append("acceptance version does not match the candidate manifest")
    if candidate.get("manifest_sha256") != sha256(arguments.manifest):
        errors.append("acceptance manifest SHA-256 does not match the exact candidate")
    if candidate.get("manifest_signature_sha256") != sha256(arguments.manifest_signature):
        errors.append("acceptance signature SHA-256 does not match the exact signed candidate")

    required_matrix = {(board, surface, os_name) for board in BOARDS for surface in SURFACES for os_name in OS_NAMES}
    seen_matrix: set[tuple[str, str, str]] = set()
    physical_architectures: set[tuple[str, str]] = set()
    scenarios: dict[tuple[str, str], set[str]] = defaultdict(set)
    for index, run in enumerate(acceptance.get("runs", [])):
        label = f"runs[{index}]"
        board = run.get("board")
        surface = run.get("surface")
        os_name = run.get("os")
        architecture = run.get("architecture")
        if (board, surface, os_name) not in required_matrix:
            errors.append(f"{label} has an unknown board/surface/OS tuple")
            continue
        if run.get("result") != "pass":
            errors.append(f"{label} is not a passing acceptance run")
        if not nonempty(run, "hardware_identity", "client_version", "tester", "date"):
            errors.append(f"{label} is missing hardware/client/tester/date evidence")
        else:
            try:
                date.fromisoformat(run["date"])
            except ValueError:
                errors.append(f"{label} date must be ISO YYYY-MM-DD")
        if (os_name, architecture) not in ARCHITECTURES:
            errors.append(f"{label} has an unsupported OS/architecture pair")
        else:
            physical_architectures.add((os_name, architecture))
        if surface == "web":
            browser = str(run.get("browser", ""))
            required_browser = "Edge" if os_name == "windows" else "Chrome"
            if required_browser not in browser or not any(character.isdigit() for character in browser):
                errors.append(f"{label} must record a stable {required_browser} version")
        scenario_results = run.get("scenarios")
        if not isinstance(scenario_results, dict) or not scenario_results:
            errors.append(f"{label} must include named scenario results")
        else:
            failed = [name for name, result in scenario_results.items() if result != "pass"]
            if failed:
                errors.append(f"{label} contains non-passing scenarios: {failed}")
            scenarios[(board, surface)].update(scenario_results)
            if surface == "web" and scenario_results.get("firefox-cli-fallback") != "pass":
                errors.append(f"{label} must prove the Firefox CLI fallback")
            if (
                surface == "web"
                and os_name == "macos"
                and scenario_results.get("safari-cli-fallback") != "pass"
            ):
                errors.append(f"{label} must prove the Safari CLI fallback")
        key = (board, surface, os_name)
        if key in seen_matrix:
            errors.append(f"duplicate matrix result for {key}")
        seen_matrix.add(key)

    missing_matrix = sorted(required_matrix - seen_matrix)
    if missing_matrix:
        errors.append(f"missing board/surface/OS runs: {missing_matrix}")
    missing_architectures = sorted(ARCHITECTURES - physical_architectures)
    if missing_architectures:
        errors.append(f"missing representative physical architectures: {missing_architectures}")
    for board in BOARDS:
        for surface in SURFACES:
            required = set(COMMON_SCENARIOS)
            if board in {"heltec-v4", "t-beam-supreme"}:
                required |= PROVISIONING_SCENARIOS
            if board == "t-echo":
                required |= TECHO_SCENARIOS
            missing = sorted(required - scenarios[(board, surface)])
            if missing:
                errors.append(f"{board}/{surface} is missing scenarios: {missing}")

    smoke_targets = set()
    for index, smoke in enumerate(acceptance.get("installation_smoke", [])):
        label = f"installation_smoke[{index}]"
        target = smoke.get("target")
        if target not in CLI_TARGETS:
            errors.append(f"{label} has an unknown published target")
            continue
        if target in smoke_targets:
            errors.append(f"duplicate installation smoke for {target}")
        smoke_targets.add(target)
        if smoke.get("result") != "pass" or not nonempty(smoke, "tester", "date", "cli_version"):
            errors.append(f"{label} is incomplete or not passing")
    missing_smokes = sorted(CLI_TARGETS - smoke_targets)
    if missing_smokes:
        errors.append(f"missing native installation/doctor smokes: {missing_smokes}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--acceptance", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--manifest-signature", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        errors = validate(arguments)
    except (OSError, json.JSONDecodeError) as error:
        print(f"acceptance validation failed: {error}", file=sys.stderr)
        return 1
    if errors:
        for error in errors:
            print(f"acceptance validation failed: {error}", file=sys.stderr)
        return 1
    print("physical flasher acceptance matrix is complete for the exact signed candidate")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
