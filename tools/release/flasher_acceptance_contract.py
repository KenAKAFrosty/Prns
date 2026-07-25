"""Authoritative physical-qualification matrix and scaffold construction."""

from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
import hashlib
from pathlib import Path
import re


SHIPPING_BOARDS = (
    "heltec-v4",
    "t-beam-supreme",
    "xiao-esp32-c6",
    "t-echo",
)
SURFACES = ("cli", "web")
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
FALLBACK_SCENARIOS = {
    "esp-cli-guidance",
    "esp-connect-unavailable",
    "no-broken-connect-action",
    "t-echo-uf2-route",
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

PER_RUN_BASELINE_SCENARIOS = {"fresh-install", "post-flash-boot"}
NOT_RUN = "NOT_RUN"
UTC_TIMESTAMP = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$")


def parse_utc_timestamp(value: object, label: str) -> datetime:
    if not isinstance(value, str) or UTC_TIMESTAMP.fullmatch(value) is None:
        raise ValueError(f"{label} must be a full UTC timestamp ending in Z")
    try:
        return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as error:
        raise ValueError(f"{label} must be a valid UTC timestamp") from error


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


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


def evidence_placeholder() -> dict:
    return {
        "reference": NOT_RUN,
        "sha256": NOT_RUN,
        "redaction": {
            "reviewer": NOT_RUN,
            "credentials_removed": False,
            "device_identifiers_removed": False,
            "local_paths_removed": False,
            "private_network_data_removed": False,
        },
    }


def scaffold(
    manifest: dict,
    manifest_path: Path,
    manifest_signature_path: Path,
    signed_bundle_path: Path,
    prerelease_published_at: str,
    tester_roster: object,
) -> dict:
    parse_utc_timestamp(prerelease_published_at, "prerelease publishedAt")
    release = manifest.get("release")
    signing = manifest.get("signing")
    raw_targets = manifest.get("targets")
    if manifest.get("schema") != 2:
        raise ValueError("manifest must use schema 2")
    if not isinstance(release, dict) or not isinstance(signing, dict):
        raise ValueError("manifest release/signing identity is malformed")
    if not isinstance(raw_targets, list):
        raise ValueError("manifest targets must be an array")
    version = release.get("version")
    channel = release.get("channel")
    commit = release.get("commit")
    key_id = signing.get("key_id")
    if (
        not isinstance(version, str)
        or not version
        or version.lower() == "next"
        or channel not in {"stable", "preview"}
        or not isinstance(commit, str)
        or len(commit) != 40
        or any(character not in "0123456789abcdef" for character in commit)
        or not isinstance(key_id, str)
        or len(key_id) != 16
        or any(character not in "0123456789abcdefABCDEF" for character in key_id)
    ):
        raise ValueError("manifest release/signing identity is not publishable")
    if len(raw_targets) != len(SHIPPING_BOARDS) or not all(
        isinstance(target, dict) and isinstance(target.get("board_slug"), str)
        for target in raw_targets
    ):
        raise ValueError("manifest must contain exactly four well-formed targets")
    targets = {
        target.get("board_slug"): target
        for target in raw_targets
    }
    if len(targets) != len(raw_targets) or set(targets) != set(SHIPPING_BOARDS):
        raise ValueError("manifest must contain exactly the four shipping boards")
    if any(
        not isinstance(target.get("display_name"), str)
        or not target["display_name"].strip()
        or target.get("transport") not in {"esp-serial", "uf2-mass-storage"}
        for target in targets.values()
    ):
        raise ValueError("manifest targets have malformed identity or transport fields")
    chip_counts = Counter(
        target.get("expected_chip")
        for target in targets.values()
        if target.get("transport") == "esp-serial"
        and isinstance(target.get("expected_chip"), str)
    )

    runs = []
    physical_assignments = getattr(tester_roster, "physical", {})
    fallback_assignments = getattr(tester_roster, "fallbacks", {})
    installation_assignments = getattr(tester_roster, "installations", {})
    for board in SHIPPING_BOARDS:
        target = targets[board]
        for surface in SURFACES:
            assignment = physical_assignments.get((board, surface))
            if assignment is None:
                raise ValueError(f"tester roster is missing {board}/{surface}")
            required = applicable_scenarios(target, surface, chip_counts)
            run = {
                "board": board,
                "surface": surface,
                "os": assignment.os_name,
                "architecture": assignment.architecture,
                "os_version": NOT_RUN,
                "hardware_identity": NOT_RUN,
                "hardware_model": target.get("display_name", NOT_RUN),
                "hardware_revision": NOT_RUN,
                "client": {
                    "name": "prns-web-flasher"
                    if surface == "web"
                    else "hopspot-flash",
                    "version": version,
                },
                "scenarios": {
                    scenario: "not-run" for scenario in sorted(required)
                },
                "result": "not-run",
                "tester": assignment.tester,
                "completed_at": NOT_RUN,
                "evidence": evidence_placeholder(),
            }
            if surface == "web":
                run["browser"] = {
                    "name": assignment.browser_name,
                    "channel": "stable",
                    "version": NOT_RUN,
                }
            runs.append(run)

    browser_fallbacks = []
    for browser, os_name in sorted(REQUIRED_FALLBACKS):
        assignment = fallback_assignments.get((browser, os_name))
        if assignment is None:
            raise ValueError(f"tester roster is missing {browser}/{os_name}")
        browser_fallbacks.append(
            {
                "os": os_name,
                "architecture": assignment.architecture,
                "os_version": NOT_RUN,
                "client": {
                    "name": "prns-web-flasher",
                    "version": version,
                },
                "browser": {
                    "name": browser,
                    "channel": "stable",
                    "version": NOT_RUN,
                },
                "scenarios": {
                    scenario: "not-run" for scenario in sorted(FALLBACK_SCENARIOS)
                },
                "result": "not-run",
                "tester": assignment.tester,
                "completed_at": NOT_RUN,
                "evidence": evidence_placeholder(),
            }
        )

    installation_smoke = []
    for target, (os_name, architecture) in CLI_TARGETS.items():
        assignment = installation_assignments.get(target)
        if assignment is None:
            raise ValueError(f"tester roster is missing {target}")
        installation_smoke.append(
            {
                "target": target,
                "os": os_name,
                "architecture": architecture,
                "os_version": NOT_RUN,
                "cli_version": version,
                "scenarios": {"install": "not-run", "version": "not-run"},
                "result": "not-run",
                "tester": assignment.tester,
                "completed_at": NOT_RUN,
                "evidence": evidence_placeholder(),
            }
        )

    return {
        "schema": 3,
        "candidate": {
            "version": version,
            "channel": channel,
            "source_commit": commit,
            "signing_key_id": key_id,
            "manifest_sha256": sha256(manifest_path),
            "manifest_signature_sha256": sha256(manifest_signature_path),
            "signed_candidate_sha256": sha256(signed_bundle_path),
            "prerelease_published_at": prerelease_published_at,
        },
        "runs": runs,
        "browser_fallbacks": browser_fallbacks,
        "installation_smoke": installation_smoke,
    }
