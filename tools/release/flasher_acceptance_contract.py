"""Authoritative physical-qualification matrix and scaffold construction."""

from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
import hashlib
from pathlib import Path
import re

from flasher_manifest import require_schema, target_artifacts


ESP_SERIAL_BOARDS = (
    "heltec-v4",
    "heltec-v4-r8",
    "t-beam-supreme",
    "xiao-esp32-c6",
)
SHIPPING_BOARDS = (
    *ESP_SERIAL_BOARDS,
    "t-echo",
    "t114",
    "t096",
    "t1000-e",
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
WEB_SERIAL_HOSTS = {
    "linux": {"aarch64", "x86_64"},
    "macos": {"aarch64", "x86_64"},
    "windows": {"x86_64"},
}
WEB_SERIAL_SCENARIOS = {
    "correct-board",
    "fresh-install",
    "one-device",
    "permission-grant",
    "post-flash-boot",
}
REQUIRED_FALLBACKS = {("safari", "macos")}
FALLBACK_SCENARIOS = {
    "esp-cli-guidance",
    "esp-connect-unavailable",
    "no-broken-connect-action",
    "t-echo-uf2-route",
    "t096-uf2-route",
    "t1000-e-recovery-uf2-route",
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
    "foundation-detection",
    "unsupported-foundation-rejection",
    "display",
    "ble",
    "lora",
}
UF2_WEB_SCENARIOS = {
    "manual-copy-flow",
    "missing-mount-guidance",
    "copy-failure-guidance",
    "reboot-guidance",
    "malformed-foundation-rejection",
    "local-only-info-file",
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
    "application-usb-enumeration",
}

NRF_SERIAL_DFU_COMMON_SCENARIOS = {
    "fresh-install",
    "update",
    "correct-board",
    "incorrect-board",
    "signed-dfu-verification",
    "corrupt-artifact",
    "signature-rejection",
    "exact-bootloader-selection",
    "reliable-dfu-transfer",
    "activation",
    "recovery-uf2-fallback",
    "lora",
    "usb",
    "post-flash-boot",
}
NRF_SERIAL_DFU_WEB_SCENARIOS = {
    "permission-denial",
    "managed-application-entry",
    "bootloader-serial-selection",
    "navigation-warning",
    "recovery-guidance",
}
NRF_SERIAL_DFU_CLI_SCENARIOS = {
    "zero-devices",
    "one-device",
    "multiple-devices",
    "port-unavailable",
    "bootloader-entry",
    "bootloader-timeout",
    "transfer-retry",
    "recovery-guidance",
}

PER_RUN_BASELINE_SCENARIOS = {"fresh-install", "post-flash-boot"}
ACCEPTANCE_SCHEMA = 5
T_ECHO_COMPATIBILITY_VARIANTS = (
    "s140-6.1.1-fwid-0x00b6",
    "s140-7.3.0-fwid-0x0123",
)
T114_COMPATIBILITY_VARIANTS = ("s140-6.1.1-fwid-0x00b6",)
T096_COMPATIBILITY_VARIANTS = ("s140-6.1.1-fwid-0x00b6",)
UF2_COMPATIBILITY_VARIANTS = {
    "t-echo": T_ECHO_COMPATIBILITY_VARIANTS,
    "t114": T114_COMPATIBILITY_VARIANTS,
    "t096": T096_COMPATIBILITY_VARIANTS,
}
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
    if transport == "nrf-serial-dfu":
        scenarios = set(NRF_SERIAL_DFU_COMMON_SCENARIOS)
        scenarios.update(
            NRF_SERIAL_DFU_WEB_SCENARIOS
            if surface == "web"
            else NRF_SERIAL_DFU_CLI_SCENARIOS
        )
        return scenarios
    return set()


def required_compatibilities(target: dict) -> tuple[str | None, ...]:
    if target.get("transport") != "uf2-mass-storage":
        return (None,)
    board = target.get("board_slug")
    expected = UF2_COMPATIBILITY_VARIANTS.get(board)
    if expected is None:
        raise ValueError(f"UF2 acceptance has no pinned compatibility matrix for {board!r}")
    labels = []
    for variant in target_artifacts(target):
        family = variant.get("softdevice_family")
        version = variant.get("softdevice_version")
        fwid = variant.get("fwid")
        if not all(isinstance(value, str) for value in (family, version, fwid)):
            raise ValueError("UF2 acceptance compatibility identity is malformed")
        labels.append(f"{family}-{version}-fwid-{fwid}")
    if tuple(labels) != expected:
        if board == "t-echo":
            raise ValueError(
                "T-Echo acceptance requires the exact S140 v6 and v7 compatibility matrix"
            )
        raise ValueError(f"{board} acceptance requires its exact pinned compatibility matrix")
    return tuple(labels)


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
    require_schema(manifest)
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
        raise ValueError(
            "manifest must contain exactly "
            f"{len(SHIPPING_BOARDS)} well-formed targets"
        )
    targets = {
        target.get("board_slug"): target
        for target in raw_targets
    }
    if len(targets) != len(raw_targets) or set(targets) != set(SHIPPING_BOARDS):
        raise ValueError("manifest must contain exactly the shipping board set")
    if any(
        not isinstance(target.get("display_name"), str)
        or not target["display_name"].strip()
        or target.get("transport")
        not in {"esp-serial", "uf2-mass-storage", "nrf-serial-dfu"}
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
    web_serial_assignments = getattr(tester_roster, "web_serial", {})
    fallback_assignments = getattr(tester_roster, "fallbacks", {})
    installation_assignments = getattr(tester_roster, "installations", {})
    for board in SHIPPING_BOARDS:
        target = targets[board]
        for surface in SURFACES:
            assignment = physical_assignments.get((board, surface))
            if assignment is None:
                raise ValueError(f"tester roster is missing {board}/{surface}")
            required = applicable_scenarios(target, surface, chip_counts)
            for compatibility in required_compatibilities(target):
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
                if compatibility is not None:
                    run["compatibility_variant"] = compatibility
                if surface == "web":
                    run["browser"] = {
                        "name": assignment.browser_name,
                        "channel": "stable",
                        "version": NOT_RUN,
                    }
                runs.append(run)

    web_serial_smoke = []
    for os_name in WEB_SERIAL_HOSTS:
        assignment = web_serial_assignments.get(os_name)
        if assignment is None:
            raise ValueError(f"tester roster is missing Firefox Web Serial {os_name}")
        target = targets[assignment.board]
        web_serial_smoke.append(
            {
                "board": assignment.board,
                "os": os_name,
                "architecture": assignment.architecture,
                "os_version": NOT_RUN,
                "hardware_identity": NOT_RUN,
                "hardware_model": target.get("display_name", NOT_RUN),
                "hardware_revision": NOT_RUN,
                "client": {
                    "name": "prns-web-flasher",
                    "version": version,
                },
                "browser": {
                    "name": "firefox",
                    "channel": "stable",
                    "version": NOT_RUN,
                },
                "scenarios": {
                    scenario: "not-run" for scenario in sorted(WEB_SERIAL_SCENARIOS)
                },
                "result": "not-run",
                "tester": assignment.tester,
                "completed_at": NOT_RUN,
                "evidence": evidence_placeholder(),
            }
        )

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
        "schema": ACCEPTANCE_SCHEMA,
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
        "web_serial_smoke": web_serial_smoke,
        "browser_fallbacks": browser_fallbacks,
        "installation_smoke": installation_smoke,
    }
