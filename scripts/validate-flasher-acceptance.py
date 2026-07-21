#!/usr/bin/env python3
"""Fail closed unless an exact signed candidate has truthful physical evidence."""

from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from datetime import date
import hashlib
import json
from pathlib import Path
import sys


SHIPPING_BOARDS = {"heltec-v4", "t-beam-supreme", "xiao-esp32-c6", "t-echo"}
SURFACES = {"web", "cli"}
OS_ARCHITECTURES = {
    ("macos", "aarch64"),
    ("macos", "x86_64"),
    ("linux", "x86_64"),
    ("linux", "aarch64"),
    ("windows", "x86_64"),
}
CLI_TARGETS = {
    "aarch64-apple-darwin": ("macos", "aarch64"),
    "x86_64-apple-darwin": ("macos", "x86_64"),
    "x86_64-unknown-linux-gnu": ("linux", "x86_64"),
    "aarch64-unknown-linux-gnu": ("linux", "aarch64"),
    "x86_64-pc-windows-msvc": ("windows", "x86_64"),
}
REQUIRED_FALLBACKS = {
    ("firefox", "macos"),
    ("firefox", "windows"),
    ("firefox", "linux"),
    ("safari", "macos"),
}

ESP_COMMON_SCENARIOS = {
    "fresh-install",
    "update",
    "correct-board",
    "incorrect-board",
    "zero-devices",
    "one-device",
    "multiple-devices",
    "sparse-write",
    "wrong-chip",
    "boot-reset-recovery",
    "disconnect-before-write",
    "disconnect-during-write",
    "disconnect-before-reset",
    "corrupt-artifact",
    "signature-rejection",
    "reset-failure",
    "post-flash-boot",
}
ESP_WEB_SCENARIOS = {"permission-denial", "device-md5-mismatch", "navigation-warning"}
ESP_CLI_SCENARIOS = {"port-unavailable", "write-verification-failure"}
PROVISIONING_SCENARIOS = {"preserve", "configure", "clear"}

UF2_COMMON_SCENARIOS = {
    "fresh-install",
    "update",
    "correct-board",
    "incorrect-board",
    "signed-uf2-verification",
    "corrupt-artifact",
    "signature-rejection",
    "post-flash-boot",
}
UF2_WEB_SCENARIOS = {
    "manual-copy-flow",
    "missing-mount-guidance",
    "copy-failure-guidance",
    "reboot-guidance",
}
UF2_CLI_SCENARIOS = {
    "zero-mounts",
    "one-mount",
    "multiple-mounts",
    "failed-copy",
    "failed-flush",
    "failed-sync",
    "mount-disappearance",
    "reboot-detection",
    "reboot-timeout",
}

TOP_LEVEL_FIELDS = {"schema", "candidate", "runs", "browser_fallbacks", "installation_smoke"}
CANDIDATE_FIELDS = {
    "version",
    "channel",
    "source_commit",
    "signing_key_id",
    "manifest_sha256",
    "manifest_signature_sha256",
}
RUN_FIELDS = {
    "board",
    "surface",
    "os",
    "architecture",
    "os_version",
    "hardware_identity",
    "hardware_model",
    "hardware_revision",
    "client",
    "browser",
    "scenarios",
    "result",
    "tester",
    "date",
    "evidence",
}
CLIENT_FIELDS = {"name", "version"}
BROWSER_FIELDS = {"name", "version"}
EVIDENCE_FIELDS = {"reference", "redaction"}
FALLBACK_FIELDS = {
    "os",
    "architecture",
    "os_version",
    "client",
    "browser",
    "result",
    "tester",
    "date",
    "evidence",
}
INSTALLATION_FIELDS = {
    "target",
    "os",
    "architecture",
    "os_version",
    "cli_version",
    "scenarios",
    "result",
    "tester",
    "date",
    "evidence",
}
PLACEHOLDER_PREFIXES = ("REPLACE", "TODO", "TBD", "UNKNOWN")


def sha256(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def is_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def is_evidence_text(value: object) -> bool:
    if not isinstance(value, str) or not value.strip() or value != value.strip():
        return False
    if len(value) > 512 or "\n" in value or "\r" in value:
        return False
    return not value.upper().startswith(PLACEHOLDER_PREFIXES)


def reject_unknown_fields(record: dict, allowed: set[str], label: str, errors: list[str]) -> None:
    unknown = sorted(set(record) - allowed)
    if unknown:
        errors.append(f"{label} contains unknown fields: {unknown}")


def require_text(record: dict, fields: set[str], label: str, errors: list[str]) -> None:
    missing = sorted(field for field in fields if not is_evidence_text(record.get(field)))
    if missing:
        errors.append(f"{label} has missing, placeholder, or malformed text fields: {missing}")


def validate_date(record: dict, label: str, errors: list[str]) -> None:
    value = record.get("date")
    if not isinstance(value, str):
        errors.append(f"{label} date must be ISO YYYY-MM-DD")
        return
    try:
        recorded = date.fromisoformat(value)
    except ValueError:
        errors.append(f"{label} date must be ISO YYYY-MM-DD")
        return
    if recorded > date.today():
        errors.append(f"{label} date cannot be in the future")


def validate_evidence(value: object, label: str, errors: list[str]) -> None:
    if not isinstance(value, dict):
        errors.append(f"{label} evidence must be an object")
        return
    reject_unknown_fields(value, EVIDENCE_FIELDS, f"{label}.evidence", errors)
    if not is_evidence_text(value.get("reference")):
        errors.append(f"{label} evidence reference is missing or still a placeholder")
    if value.get("redaction") != "reviewed":
        errors.append(f"{label} evidence redaction must be 'reviewed'")


def validate_client(
    value: object, expected_name: str, version: str, label: str, errors: list[str]
) -> None:
    if not isinstance(value, dict):
        errors.append(f"{label} client must be an object")
        return
    reject_unknown_fields(value, CLIENT_FIELDS, f"{label}.client", errors)
    if value != {"name": expected_name, "version": version}:
        errors.append(
            f"{label} client must identify {expected_name} at exact candidate version {version}"
        )


def validate_browser(
    value: object, expected_name: str, label: str, errors: list[str]
) -> tuple[str | None, str | None]:
    if not isinstance(value, dict):
        errors.append(f"{label} browser must be an object")
        return None, None
    reject_unknown_fields(value, BROWSER_FIELDS, f"{label}.browser", errors)
    name = value.get("name")
    version = value.get("version")
    if name != expected_name or not is_evidence_text(version) or not any(
        character.isdigit() for character in str(version)
    ):
        errors.append(f"{label} must record exact {expected_name} browser version")
    return name if isinstance(name, str) else None, version if isinstance(version, str) else None


def validate_scenarios(
    value: object, allowed: set[str], label: str, errors: list[str]
) -> set[str]:
    if not isinstance(value, dict) or not value:
        errors.append(f"{label} must include named scenario results")
        return set()
    unknown = sorted(set(value) - allowed)
    if unknown:
        errors.append(f"{label} claims scenarios that do not apply: {unknown}")
    failed = sorted(name for name, result in value.items() if result != "pass")
    if failed:
        errors.append(f"{label} contains non-passing scenarios: {failed}")
    return set(value) & allowed


def manifest_targets(manifest: dict, errors: list[str]) -> dict[str, dict]:
    raw_targets = manifest.get("targets")
    if not isinstance(raw_targets, list):
        errors.append("candidate manifest targets must be an array")
        return {}
    targets: dict[str, dict] = {}
    for index, target in enumerate(raw_targets):
        if not isinstance(target, dict) or not isinstance(target.get("board_slug"), str):
            errors.append(f"candidate manifest targets[{index}] is malformed")
            continue
        board = target["board_slug"]
        if board in targets:
            errors.append(f"candidate manifest duplicates board {board}")
        targets[board] = target
    if set(targets) != SHIPPING_BOARDS:
        errors.append("candidate manifest does not contain exactly the four shipping boards")
    return targets


def applicable_scenarios(
    target: dict, surface: str, chip_counts: Counter[str]
) -> set[str]:
    transport = target.get("transport")
    if transport == "esp-serial":
        scenarios = set(ESP_COMMON_SCENARIOS)
        scenarios.update(ESP_WEB_SCENARIOS if surface == "web" else ESP_CLI_SCENARIOS)
        chip = target.get("expected_chip")
        if isinstance(chip, str) and chip_counts[chip] > 1:
            scenarios.add("same-chip-board-confirmation")
        if target.get("provisioning") is not None:
            scenarios.update(PROVISIONING_SCENARIOS)
        return scenarios
    if transport == "uf2-mass-storage":
        scenarios = set(UF2_COMMON_SCENARIOS)
        scenarios.update(UF2_WEB_SCENARIOS if surface == "web" else UF2_CLI_SCENARIOS)
        return scenarios
    return set()


def validate_candidate_identity(
    acceptance: dict,
    manifest: dict,
    manifest_path: Path,
    signature_path: Path,
    errors: list[str],
) -> tuple[str, dict[str, dict]]:
    release = manifest.get("release") if isinstance(manifest.get("release"), dict) else {}
    signing = manifest.get("signing") if isinstance(manifest.get("signing"), dict) else {}
    version = release.get("version") if isinstance(release.get("version"), str) else ""
    candidate = acceptance.get("candidate")
    if not isinstance(candidate, dict):
        errors.append("acceptance candidate must be an object")
        return version, manifest_targets(manifest, errors)
    reject_unknown_fields(candidate, CANDIDATE_FIELDS, "candidate", errors)
    expected = {
        "version": version,
        "channel": release.get("channel"),
        "source_commit": release.get("commit"),
        "signing_key_id": signing.get("key_id"),
        "manifest_sha256": sha256(manifest_path),
        "manifest_signature_sha256": sha256(signature_path),
    }
    for field, expected_value in expected.items():
        actual = candidate.get(field)
        if field == "signing_key_id" and isinstance(actual, str) and isinstance(expected_value, str):
            matches = actual.upper() == expected_value.upper()
        else:
            matches = actual == expected_value
        if not matches:
            errors.append(f"acceptance {field} does not match the exact signed manifest")
    require_text(candidate, CANDIDATE_FIELDS, "candidate", errors)
    if candidate.get("channel") not in {"stable", "preview"}:
        errors.append("acceptance channel must be stable or preview")
    if candidate.get("version") == "next":
        errors.append("acceptance version cannot be next")
    if not is_sha256(candidate.get("manifest_sha256")) or not is_sha256(
        candidate.get("manifest_signature_sha256")
    ):
        errors.append("acceptance candidate hashes must be lowercase SHA-256 values")
    source_commit = candidate.get("source_commit")
    if not (
        isinstance(source_commit, str)
        and len(source_commit) == 40
        and all(character in "0123456789abcdef" for character in source_commit)
    ):
        errors.append("acceptance source_commit must be a lowercase full Git commit")
    key_id = candidate.get("signing_key_id")
    if not (
        isinstance(key_id, str)
        and len(key_id) == 16
        and all(character in "0123456789abcdefABCDEF" for character in key_id)
    ):
        errors.append("acceptance signing_key_id must be 16 hexadecimal digits")
    return version, manifest_targets(manifest, errors)


def validate_runs(
    acceptance: dict,
    targets: dict[str, dict],
    version: str,
    errors: list[str],
) -> None:
    required_matrix = {
        (board, surface, os_name)
        for board in SHIPPING_BOARDS
        for surface in SURFACES
        for os_name in {"macos", "windows", "linux"}
    }
    seen_matrix: set[tuple[str, str, str]] = set()
    physical_architectures: set[tuple[str, str]] = set()
    coverage: dict[tuple[str, str], set[str]] = defaultdict(set)
    chip_counts = Counter(
        target.get("expected_chip")
        for target in targets.values()
        if target.get("transport") == "esp-serial" and isinstance(target.get("expected_chip"), str)
    )
    runs = acceptance.get("runs")
    if not isinstance(runs, list):
        errors.append("acceptance runs must be an array")
        return
    for index, run in enumerate(runs):
        label = f"runs[{index}]"
        if not isinstance(run, dict):
            errors.append(f"{label} must be an object")
            continue
        reject_unknown_fields(run, RUN_FIELDS, label, errors)
        board = run.get("board")
        surface = run.get("surface")
        os_name = run.get("os")
        architecture = run.get("architecture")
        if not all(isinstance(value, str) for value in (board, surface, os_name, architecture)):
            errors.append(f"{label} board, surface, OS, and architecture must be strings")
            continue
        key = (board, surface, os_name)
        if key not in required_matrix:
            errors.append(f"{label} has an unknown board/surface/OS tuple")
            continue
        if key in seen_matrix:
            errors.append(f"duplicate matrix result for {key}")
        seen_matrix.add(key)
        if (os_name, architecture) not in OS_ARCHITECTURES:
            errors.append(f"{label} has an unsupported OS/architecture pair")
        else:
            physical_architectures.add((os_name, architecture))
        if run.get("result") != "pass":
            errors.append(f"{label} is not a passing acceptance run")
        require_text(
            run,
            {
                "os_version",
                "hardware_identity",
                "hardware_model",
                "hardware_revision",
                "tester",
            },
            label,
            errors,
        )
        validate_date(run, label, errors)
        validate_evidence(run.get("evidence"), label, errors)
        target = targets.get(str(board), {})
        if run.get("hardware_model") != target.get("display_name"):
            errors.append(f"{label} hardware_model differs from the signed manifest")
        expected_client = "prns-web-flasher" if surface == "web" else "hopspot-flash"
        validate_client(run.get("client"), expected_client, version, label, errors)
        if surface == "web":
            expected_browser = "edge" if os_name == "windows" else "chrome"
            validate_browser(run.get("browser"), expected_browser, label, errors)
        elif "browser" in run:
            errors.append(f"{label} CLI run must not claim browser evidence")
        allowed = applicable_scenarios(target, str(surface), chip_counts)
        if not allowed:
            errors.append(f"{label} target has an unsupported transport")
        coverage[(str(board), str(surface))].update(
            validate_scenarios(run.get("scenarios"), allowed, label, errors)
        )

    missing_matrix = sorted(required_matrix - seen_matrix)
    if missing_matrix:
        errors.append(f"missing board/surface/OS runs: {missing_matrix}")
    missing_architectures = sorted(OS_ARCHITECTURES - physical_architectures)
    if missing_architectures:
        errors.append(f"missing representative physical architectures: {missing_architectures}")
    for board, target in targets.items():
        for surface in sorted(SURFACES):
            required = applicable_scenarios(target, surface, chip_counts)
            missing = sorted(required - coverage[(board, surface)])
            if missing:
                errors.append(f"{board}/{surface} is missing scenarios: {missing}")


def validate_fallbacks(acceptance: dict, version: str, errors: list[str]) -> None:
    entries = acceptance.get("browser_fallbacks")
    if not isinstance(entries, list):
        errors.append("acceptance browser_fallbacks must be an array")
        return
    seen: set[tuple[str, str]] = set()
    for index, entry in enumerate(entries):
        label = f"browser_fallbacks[{index}]"
        if not isinstance(entry, dict):
            errors.append(f"{label} must be an object")
            continue
        reject_unknown_fields(entry, FALLBACK_FIELDS, label, errors)
        os_name = entry.get("os")
        architecture = entry.get("architecture")
        if not isinstance(os_name, str) or not isinstance(architecture, str):
            errors.append(f"{label} OS and architecture must be strings")
            continue
        if (os_name, architecture) not in OS_ARCHITECTURES:
            errors.append(f"{label} has an unsupported OS/architecture pair")
        require_text(entry, {"os_version", "tester"}, label, errors)
        validate_date(entry, label, errors)
        validate_evidence(entry.get("evidence"), label, errors)
        validate_client(entry.get("client"), "prns-web-flasher", version, label, errors)
        browser = entry.get("browser")
        raw_browser_name = browser.get("name") if isinstance(browser, dict) else None
        browser_name = raw_browser_name if isinstance(raw_browser_name, str) else None
        key = (browser_name, os_name)
        expected_name = browser_name if key in REQUIRED_FALLBACKS else "unsupported-browser"
        validate_browser(browser, expected_name, label, errors)
        if key not in REQUIRED_FALLBACKS:
            errors.append(f"{label} is not a required Safari/Firefox fallback")
        elif key in seen:
            errors.append(f"duplicate browser fallback for {key}")
        seen.add(key)
        if entry.get("result") != "pass":
            errors.append(f"{label} is not a passing fallback check")
    missing = sorted(REQUIRED_FALLBACKS - seen)
    if missing:
        errors.append(f"missing browser fallback checks: {missing}")


def validate_installation_smokes(acceptance: dict, version: str, errors: list[str]) -> None:
    entries = acceptance.get("installation_smoke")
    if not isinstance(entries, list):
        errors.append("acceptance installation_smoke must be an array")
        return
    seen: set[str] = set()
    for index, entry in enumerate(entries):
        label = f"installation_smoke[{index}]"
        if not isinstance(entry, dict):
            errors.append(f"{label} must be an object")
            continue
        reject_unknown_fields(entry, INSTALLATION_FIELDS, label, errors)
        target = entry.get("target")
        if not isinstance(target, str) or target not in CLI_TARGETS:
            errors.append(f"{label} has an unknown published target")
            continue
        if target in seen:
            errors.append(f"duplicate installation smoke for {target}")
        seen.add(target)
        expected_host = CLI_TARGETS[target]
        if (entry.get("os"), entry.get("architecture")) != expected_host:
            errors.append(f"{label} host does not match target {target}")
        if entry.get("cli_version") != version:
            errors.append(f"{label} CLI version differs from the exact candidate")
        if entry.get("result") != "pass":
            errors.append(f"{label} is not a passing installation/doctor smoke")
        require_text(entry, {"os_version", "tester"}, label, errors)
        validate_date(entry, label, errors)
        validate_evidence(entry.get("evidence"), label, errors)
        validate_scenarios(entry.get("scenarios"), {"install", "doctor"}, label, errors)
        if isinstance(entry.get("scenarios"), dict) and set(entry["scenarios"]) != {
            "install",
            "doctor",
        }:
            errors.append(f"{label} must prove both install and doctor")
    missing = sorted(set(CLI_TARGETS) - seen)
    if missing:
        errors.append(f"missing native installation/doctor smokes: {missing}")


def validate(arguments: argparse.Namespace) -> list[str]:
    errors: list[str] = []
    acceptance = json.loads(arguments.acceptance.read_text(encoding="utf-8"))
    manifest = json.loads(arguments.manifest.read_text(encoding="utf-8"))
    if not isinstance(acceptance, dict):
        return ["acceptance document must be a JSON object"]
    if not isinstance(manifest, dict):
        return ["candidate manifest must be a JSON object"]
    reject_unknown_fields(acceptance, TOP_LEVEL_FIELDS, "acceptance", errors)
    if acceptance.get("schema") != 2:
        errors.append("acceptance schema must be 2")
    version, targets = validate_candidate_identity(
        acceptance, manifest, arguments.manifest, arguments.manifest_signature, errors
    )
    validate_runs(acceptance, targets, version, errors)
    validate_fallbacks(acceptance, version, errors)
    validate_installation_smokes(acceptance, version, errors)
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
