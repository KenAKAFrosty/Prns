#!/usr/bin/env python3
"""Fail closed unless an exact signed candidate has truthful physical evidence."""

from __future__ import annotations

import argparse
from collections import Counter
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import sys
from typing import NamedTuple

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from flasher_acceptance_contract import (  # noqa: E402
    ACCEPTANCE_SCHEMA,
    CLI_TARGETS,
    ESP_SERIAL_BOARDS,
    FALLBACK_SCENARIOS,
    OS_ARCHITECTURES,
    REQUIRED_FALLBACKS,
    SHIPPING_BOARDS,
    SURFACES,
    WEB_SERIAL_HOSTS,
    WEB_SERIAL_SCENARIOS,
    applicable_scenarios,
    parse_utc_timestamp,
    required_compatibilities,
    sha256,
)
from flasher_tester_roster import (  # noqa: E402
    FallbackAssignment,
    InstallationAssignment,
    PhysicalAssignment,
    TesterRoster,
    WebSerialAssignment,
    validate_roster,
)
from flasher_manifest import require_schema
from flasher_hotfix import HotfixSpec, verify_candidate as verify_hotfix_candidate

TOP_LEVEL_FIELDS = {
    "schema",
    "candidate",
    "runs",
    "web_serial_smoke",
    "browser_fallbacks",
    "installation_smoke",
}
MAINTAINER_OVERRIDE_SCHEMA = 4
MAINTAINER_OVERRIDE_VERSION = "0.3.7"
HOTFIX_ACCEPTANCE_SCHEMA = 6
OVERRIDE_TOP_LEVEL_FIELDS = {"schema", "candidate", "maintainer_override"}
OVERRIDE_FIELDS = {"basis", "approved_by", "approved_at"}
CANDIDATE_FIELDS = {
    "version",
    "channel",
    "source_commit",
    "signing_key_id",
    "manifest_sha256",
    "manifest_signature_sha256",
    "signed_candidate_sha256",
    "prerelease_published_at",
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
    "completed_at",
    "evidence",
    "compatibility_variant",
}
CLIENT_FIELDS = {"name", "version"}
BROWSER_FIELDS = {"name", "channel", "version"}
EVIDENCE_FIELDS = {"reference", "sha256", "redaction"}
REDACTION_FIELDS = {
    "reviewer",
    "credentials_removed",
    "device_identifiers_removed",
    "local_paths_removed",
    "private_network_data_removed",
}
FALLBACK_FIELDS = {
    "os",
    "architecture",
    "os_version",
    "client",
    "browser",
    "scenarios",
    "result",
    "tester",
    "completed_at",
    "evidence",
}
WEB_SERIAL_FIELDS = {
    "board",
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
    "completed_at",
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
    "completed_at",
    "evidence",
}
HOTFIX_TOP_LEVEL_FIELDS = {
    "schema",
    "candidate",
    "hotfix",
    "runs",
    "hardware_deferrals",
}
HOTFIX_IDENTITY_FIELDS = {
    "version",
    "base_version",
    "base_source_commit",
    "base_manifest_sha256",
    "base_release_record_sha256",
    "base_signed_candidate_sha256",
    "changed_boards",
    "physical_boards",
    "deferred_hardware",
    "summary",
}
HOTFIX_RUN_FIELDS = RUN_FIELDS | {"checks"}
HOTFIX_DEFERRAL_FIELDS = {
    "board",
    "basis",
    "follow_up",
    "approved_by",
    "approved_at",
}
PLACEHOLDER_PREFIXES = ("REPLACE", "TODO", "TBD", "UNKNOWN", "NOT_RUN", "NOT-RUN")
EVIDENCE_REFERENCE = re.compile(r"^artifact://qualification/([0-9a-f]{64})$")
BROWSER_VERSION = re.compile(
    r"^[1-9][0-9]*(?:\.(?:0|[1-9][0-9]*)){1,3}$"
)


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


def validate_completed_at(
    record: dict,
    label: str,
    prerelease_published_at: datetime,
    now: datetime,
    errors: list[str],
) -> None:
    try:
        completed_at = parse_utc_timestamp(record.get("completed_at"), f"{label} completed_at")
    except ValueError as error:
        errors.append(str(error))
        return
    if completed_at < prerelease_published_at:
        errors.append(f"{label} completed_at predates the exact public prerelease")
    if completed_at > now:
        errors.append(f"{label} completed_at cannot be in the future")


class EvidenceReference(NamedTuple):
    digest: str

    @classmethod
    def parse(cls, value: object) -> EvidenceReference:
        if not isinstance(value, str):
            raise ValueError("evidence reference is missing or malformed")
        matched = EVIDENCE_REFERENCE.fullmatch(value)
        if matched is None:
            raise ValueError(
                "evidence reference must be artifact://qualification/LOWERCASE_SHA256"
            )
        return cls(matched.group(1))


class EvidenceStore:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.referenced_digests: set[str] = set()
        self.verified_digests: set[str] = set()

    def validate(
        self,
        reference: EvidenceReference,
        declared_digest: str,
        label: str,
        errors: list[str],
    ) -> None:
        self.referenced_digests.add(reference.digest)
        if reference.digest != declared_digest:
            errors.append(f"{label} evidence reference and declared sha256 differ")
            return
        if reference.digest in self.verified_digests:
            return
        path = self.root / reference.digest
        if path.is_symlink() or not path.is_file():
            errors.append(f"{label} evidence object is missing from the offline evidence root")
            return
        try:
            if path.stat().st_size == 0:
                errors.append(f"{label} evidence object is empty")
                return
            actual_digest = sha256(path)
        except OSError as error:
            errors.append(f"{label} evidence object cannot be read: {error}")
            return
        if actual_digest != declared_digest:
            errors.append(f"{label} evidence bytes do not match the declared sha256")
            return
        self.verified_digests.add(reference.digest)

    def validate_inventory(self, errors: list[str]) -> None:
        if self.root.is_symlink() or not self.root.is_dir():
            errors.append("offline qualification evidence root is missing or is not a directory")
            return
        actual: set[str] = set()
        try:
            entries = list(self.root.iterdir())
        except OSError as error:
            errors.append(f"offline qualification evidence root cannot be read: {error}")
            return
        for entry in entries:
            if (
                EVIDENCE_REFERENCE.fullmatch(f"artifact://qualification/{entry.name}") is None
                or entry.is_symlink()
                or not entry.is_file()
            ):
                errors.append(
                    "offline qualification evidence root must contain only regular files named by lowercase SHA-256"
                )
                continue
            actual.add(entry.name)
        missing = sorted(self.referenced_digests - actual)
        unexpected = sorted(actual - self.referenced_digests)
        if missing:
            errors.append(f"offline qualification evidence root is missing objects: {missing}")
        if unexpected:
            errors.append(f"offline qualification evidence root contains unreferenced objects: {unexpected}")

    def validate_override_inventory(self, errors: list[str]) -> None:
        """Validate every supplemental object without claiming schema-5 matrix coverage."""
        if self.root.is_symlink() or not self.root.is_dir():
            errors.append("offline qualification evidence root is missing or is not a directory")
            return
        try:
            entries = list(self.root.iterdir())
        except OSError as error:
            errors.append(f"offline qualification evidence root cannot be read: {error}")
            return
        if not entries:
            errors.append("maintainer override requires nonempty supplemental evidence")
            return
        for entry in entries:
            if (
                EVIDENCE_REFERENCE.fullmatch(f"artifact://qualification/{entry.name}") is None
                or entry.is_symlink()
                or not entry.is_file()
            ):
                errors.append(
                    "offline qualification evidence root must contain only regular files named by lowercase SHA-256"
                )
                continue
            try:
                if entry.stat().st_size == 0:
                    errors.append(f"supplemental evidence object is empty: {entry.name}")
                elif sha256(entry) != entry.name:
                    errors.append(
                        f"supplemental evidence object name differs from its bytes: {entry.name}"
                    )
            except OSError as error:
                errors.append(f"supplemental evidence object cannot be read: {error}")


def validate_evidence(
    value: object,
    label: str,
    evidence_store: EvidenceStore,
    errors: list[str],
) -> None:
    if not isinstance(value, dict):
        errors.append(f"{label} evidence must be an object")
        return
    reject_unknown_fields(value, EVIDENCE_FIELDS, f"{label}.evidence", errors)
    reference = value.get("reference")
    evidence_sha256 = value.get("sha256")
    parsed_reference: EvidenceReference | None = None
    try:
        parsed_reference = EvidenceReference.parse(reference)
    except ValueError as error:
        errors.append(f"{label} {error}")
    if not is_sha256(evidence_sha256):
        errors.append(f"{label} evidence sha256 must be a lowercase SHA-256 value")
    elif parsed_reference is not None:
        evidence_store.validate(parsed_reference, evidence_sha256, label, errors)
    redaction = value.get("redaction")
    if not isinstance(redaction, dict):
        errors.append(f"{label} evidence redaction must be an object")
        return
    reject_unknown_fields(redaction, REDACTION_FIELDS, f"{label}.evidence.redaction", errors)
    if not is_evidence_text(redaction.get("reviewer")):
        errors.append(f"{label} evidence redaction reviewer is missing or still a placeholder")
    checks = REDACTION_FIELDS - {"reviewer"}
    failed = sorted(field for field in checks if redaction.get(field) is not True)
    if failed:
        errors.append(f"{label} evidence redaction checks are not complete: {failed}")


def validate_assignment(
    record: dict,
    assignment: (
        PhysicalAssignment
        | WebSerialAssignment
        | FallbackAssignment
        | InstallationAssignment
        | None
    ),
    label: str,
    errors: list[str],
) -> None:
    if assignment is None:
        errors.append(f"{label} has no assignment in the exact signed tester roster")
        return
    if record.get("tester") != assignment.tester:
        errors.append(f"{label} tester differs from the exact signed tester roster assignment")


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
    channel = value.get("channel")
    version = value.get("version")
    if (
        name != expected_name
        or not isinstance(version, str)
        or len(version) > 64
        or BROWSER_VERSION.fullmatch(version) is None
    ):
        errors.append(f"{label} must record exact {expected_name} browser version")
    if channel != "stable":
        errors.append(f"{label} browser channel must be stable")
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
    return {
        name for name, result in value.items() if name in allowed and result == "pass"
    }


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
    if set(targets) != set(SHIPPING_BOARDS):
        errors.append("candidate manifest does not contain exactly the shipping board set")
    return targets


def validate_candidate_identity(
    acceptance: dict,
    manifest: dict,
    manifest_path: Path,
    signature_path: Path,
    signed_bundle_path: Path,
    prerelease_published_at: str,
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
        "signed_candidate_sha256": sha256(signed_bundle_path),
        "prerelease_published_at": prerelease_published_at,
    }
    for field, expected_value in expected.items():
        actual = candidate.get(field)
        if field == "signing_key_id" and isinstance(actual, str) and isinstance(expected_value, str):
            matches = actual.upper() == expected_value.upper()
        else:
            matches = actual == expected_value
        if not matches:
            boundary = (
                "signed candidate archive"
                if field == "signed_candidate_sha256"
                else "signed manifest"
            )
            errors.append(f"acceptance {field} does not match the exact {boundary}")
    require_text(candidate, CANDIDATE_FIELDS, "candidate", errors)
    if candidate.get("channel") not in {"stable", "preview"}:
        errors.append("acceptance channel must be stable or preview")
    if candidate.get("version") == "next":
        errors.append("acceptance version cannot be next")
    if not all(
        is_sha256(candidate.get(field))
        for field in (
            "manifest_sha256",
            "manifest_signature_sha256",
            "signed_candidate_sha256",
        )
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
    roster: TesterRoster,
    evidence_store: EvidenceStore,
    prerelease_published_at: datetime,
    now: datetime,
    errors: list[str],
) -> None:
    try:
        required_matrix = {
            (board, surface, compatibility)
            for board in SHIPPING_BOARDS
            for surface in SURFACES
            for compatibility in required_compatibilities(targets.get(board, {}))
        }
    except ValueError as error:
        errors.append(str(error))
        return
    seen_matrix: set[tuple[str, str, str | None]] = set()
    t_echo_evidence: set[str] = set()
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
        compatibility = run.get("compatibility_variant")
        if compatibility is not None and not isinstance(compatibility, str):
            errors.append(f"{label} compatibility_variant must be a string")
            continue
        key = (board, surface, compatibility)
        if key not in required_matrix:
            errors.append(f"{label} has an unknown board/surface/compatibility tuple")
            continue
        if key in seen_matrix:
            errors.append(f"duplicate matrix result for {key}")
        seen_matrix.add(key)
        assignment = roster.physical.get((board, surface))
        if (os_name, architecture) not in OS_ARCHITECTURES:
            errors.append(f"{label} has an unsupported OS/architecture pair")
        if assignment is not None and (
            os_name,
            architecture,
        ) != (assignment.os_name, assignment.architecture):
            errors.append(f"{label} host differs from the exact signed tester roster assignment")
        validate_assignment(run, assignment, label, errors)
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
        validate_completed_at(run, label, prerelease_published_at, now, errors)
        validate_evidence(run.get("evidence"), label, evidence_store, errors)
        if board == "t-echo" and isinstance(run.get("evidence"), dict):
            evidence_digest = run["evidence"].get("sha256")
            if isinstance(evidence_digest, str):
                if evidence_digest in t_echo_evidence:
                    errors.append(f"{label} reuses T-Echo compatibility evidence")
                t_echo_evidence.add(evidence_digest)
        target = targets.get(str(board), {})
        if run.get("hardware_model") != target.get("display_name"):
            errors.append(f"{label} hardware_model differs from the signed manifest")
        expected_client = "prns-web-flasher" if surface == "web" else "hopspot-flash"
        validate_client(run.get("client"), expected_client, version, label, errors)
        if surface == "web":
            expected_browser = (
                assignment.browser_name
                if assignment is not None and assignment.browser_name is not None
                else "unsupported-browser"
            )
            validate_browser(run.get("browser"), expected_browser, label, errors)
        elif "browser" in run:
            errors.append(f"{label} CLI run must not claim browser evidence")
        allowed = applicable_scenarios(target, str(surface), chip_counts)
        if not allowed:
            errors.append(f"{label} target has an unsupported transport")
        observed = validate_scenarios(run.get("scenarios"), allowed, label, errors)
        missing = sorted(allowed - observed)
        if missing:
            errors.append(f"{label} is missing applicable scenarios: {missing}")

    missing_matrix = sorted(required_matrix - seen_matrix)
    if missing_matrix:
        errors.append(f"missing board/surface/compatibility runs: {missing_matrix}")


def validate_fallbacks(
    acceptance: dict,
    version: str,
    roster: TesterRoster,
    evidence_store: EvidenceStore,
    prerelease_published_at: datetime,
    now: datetime,
    errors: list[str],
) -> None:
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
        browser = entry.get("browser")
        raw_browser_name = browser.get("name") if isinstance(browser, dict) else None
        assignment = roster.fallbacks.get((str(raw_browser_name), str(os_name)))
        if (os_name, architecture) not in OS_ARCHITECTURES:
            errors.append(f"{label} has an unsupported OS/architecture pair")
        if assignment is not None and (
            os_name,
            architecture,
        ) != (assignment.os_name, assignment.architecture):
            errors.append(f"{label} host differs from the exact signed tester roster assignment")
        validate_assignment(entry, assignment, label, errors)
        require_text(entry, {"os_version", "tester"}, label, errors)
        validate_completed_at(entry, label, prerelease_published_at, now, errors)
        validate_evidence(entry.get("evidence"), label, evidence_store, errors)
        validate_client(entry.get("client"), "prns-web-flasher", version, label, errors)
        browser_name = raw_browser_name if isinstance(raw_browser_name, str) else None
        key = (browser_name, os_name)
        expected_name = browser_name if key in REQUIRED_FALLBACKS else "unsupported-browser"
        validate_browser(browser, expected_name, label, errors)
        if key not in REQUIRED_FALLBACKS:
            errors.append(f"{label} is not the required Safari fallback")
        elif key in seen:
            errors.append(f"duplicate browser fallback for {key}")
        seen.add(key)
        if entry.get("result") != "pass":
            errors.append(f"{label} is not a passing fallback check")
        observed = validate_scenarios(
            entry.get("scenarios"), FALLBACK_SCENARIOS, label, errors
        )
        missing_scenarios = sorted(FALLBACK_SCENARIOS - observed)
        if missing_scenarios:
            errors.append(f"{label} is missing fallback scenarios: {missing_scenarios}")
    missing = sorted(REQUIRED_FALLBACKS - seen)
    if missing:
        errors.append(f"missing browser fallback checks: {missing}")


def validate_web_serial_smokes(
    acceptance: dict,
    targets: dict[str, dict],
    version: str,
    roster: TesterRoster,
    evidence_store: EvidenceStore,
    prerelease_published_at: datetime,
    now: datetime,
    errors: list[str],
) -> None:
    entries = acceptance.get("web_serial_smoke")
    if not isinstance(entries, list):
        errors.append("acceptance web_serial_smoke must be an array")
        return
    seen: set[str] = set()
    evidence_digests: set[str] = set()
    for index, entry in enumerate(entries):
        label = f"web_serial_smoke[{index}]"
        if not isinstance(entry, dict):
            errors.append(f"{label} must be an object")
            continue
        reject_unknown_fields(entry, WEB_SERIAL_FIELDS, label, errors)
        board = entry.get("board")
        os_name = entry.get("os")
        architecture = entry.get("architecture")
        if not all(isinstance(value, str) for value in (board, os_name, architecture)):
            errors.append(f"{label} board, OS, and architecture must be strings")
            continue
        if board not in ESP_SERIAL_BOARDS:
            errors.append(f"{label} board must be an eligible shipping ESP-serial board")
        target = targets.get(board, {})
        if target.get("transport") != "esp-serial":
            errors.append(f"{label} cannot use a UF2 or unsupported board")
        supported_architectures = WEB_SERIAL_HOSTS.get(os_name)
        if supported_architectures is None:
            errors.append(f"{label} OS is not a required Firefox Web Serial host")
        elif architecture not in supported_architectures:
            errors.append(f"{label} architecture does not match its Firefox Web Serial host")
        if os_name in seen:
            errors.append(f"duplicate Firefox Web Serial smoke for {os_name}")
        seen.add(os_name)
        assignment = roster.web_serial.get(os_name)
        if assignment is not None and (
            board,
            os_name,
            architecture,
        ) != (assignment.board, assignment.os_name, assignment.architecture):
            errors.append(
                f"{label} board or host differs from the exact signed tester roster assignment"
            )
        validate_assignment(entry, assignment, label, errors)
        require_text(
            entry,
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
        if entry.get("hardware_model") != target.get("display_name"):
            errors.append(f"{label} hardware_model differs from the signed manifest")
        validate_client(entry.get("client"), "prns-web-flasher", version, label, errors)
        validate_browser(entry.get("browser"), "firefox", label, errors)
        validate_completed_at(entry, label, prerelease_published_at, now, errors)
        validate_evidence(entry.get("evidence"), label, evidence_store, errors)
        evidence_value = entry.get("evidence")
        evidence_digest = (
            evidence_value.get("sha256")
            if isinstance(evidence_value, dict)
            else None
        )
        if isinstance(evidence_digest, str):
            if evidence_digest in evidence_digests:
                errors.append(f"{label} reuses Firefox Web Serial evidence")
            evidence_digests.add(evidence_digest)
        if entry.get("result") != "pass":
            errors.append(f"{label} is not a passing Firefox Web Serial smoke")
        observed = validate_scenarios(
            entry.get("scenarios"), WEB_SERIAL_SCENARIOS, label, errors
        )
        missing_scenarios = sorted(WEB_SERIAL_SCENARIOS - observed)
        if missing_scenarios:
            errors.append(
                f"{label} is missing Firefox Web Serial scenarios: {missing_scenarios}"
            )
    missing = sorted(set(WEB_SERIAL_HOSTS) - seen)
    if missing:
        errors.append(f"missing Firefox Web Serial smokes: {missing}")
    if len(entries) != len(WEB_SERIAL_HOSTS):
        errors.append("acceptance must contain exactly three Firefox Web Serial smokes")


def validate_installation_smokes(
    acceptance: dict,
    version: str,
    roster: TesterRoster,
    evidence_store: EvidenceStore,
    prerelease_published_at: datetime,
    now: datetime,
    errors: list[str],
) -> None:
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
        assignment = roster.installations.get(target)
        if assignment is not None and (
            entry.get("os"),
            entry.get("architecture"),
        ) != (assignment.os_name, assignment.architecture):
            errors.append(f"{label} host differs from the exact signed tester roster assignment")
        validate_assignment(entry, assignment, label, errors)
        if entry.get("cli_version") != version:
            errors.append(f"{label} CLI version differs from the exact candidate")
        if entry.get("result") != "pass":
            errors.append(f"{label} is not a passing installation/version smoke")
        require_text(entry, {"os_version", "tester"}, label, errors)
        validate_completed_at(entry, label, prerelease_published_at, now, errors)
        validate_evidence(entry.get("evidence"), label, evidence_store, errors)
        validate_scenarios(entry.get("scenarios"), {"install", "version"}, label, errors)
        if isinstance(entry.get("scenarios"), dict) and set(entry["scenarios"]) != {
            "install",
            "version",
        }:
            errors.append(f"{label} must prove both install and exact version")
    missing = sorted(set(CLI_TARGETS) - seen)
    if missing:
        errors.append(f"missing native installation/version smokes: {missing}")


def validate_maintainer_override(
    acceptance: dict,
    raw_roster: object,
    manifest: dict,
    arguments: argparse.Namespace,
    prerelease_published_at: datetime,
    now: datetime,
) -> list[str]:
    errors: list[str] = []
    reject_unknown_fields(acceptance, OVERRIDE_TOP_LEVEL_FIELDS, "acceptance", errors)
    version, _ = validate_candidate_identity(
        acceptance,
        manifest,
        arguments.manifest,
        arguments.manifest_signature,
        arguments.signed_bundle,
        arguments.prerelease_published_at,
        errors,
    )
    if version != MAINTAINER_OVERRIDE_VERSION:
        errors.append(
            f"maintainer override is restricted to version {MAINTAINER_OVERRIDE_VERSION}"
        )
    override = acceptance.get("maintainer_override")
    if not isinstance(override, dict):
        errors.append("maintainer_override must be an object")
        return errors
    reject_unknown_fields(override, OVERRIDE_FIELDS, "maintainer_override", errors)
    if not is_evidence_text(override.get("basis")):
        errors.append("maintainer_override basis must state the approval grounds")
    release_owner = raw_roster.get("release_owner") if isinstance(raw_roster, dict) else None
    approved_by = override.get("approved_by")
    if not is_evidence_text(approved_by) or approved_by != release_owner:
        errors.append(
            "maintainer_override approved_by must be the signed roster release_owner"
        )
    try:
        approved_at = parse_utc_timestamp(
            override.get("approved_at"), "maintainer_override approved_at"
        )
    except ValueError as error:
        errors.append(str(error))
    else:
        if approved_at < prerelease_published_at:
            errors.append(
                "maintainer_override approved_at predates the exact public prerelease"
            )
        if approved_at > now:
            errors.append("maintainer_override approved_at cannot be in the future")
    EvidenceStore(arguments.evidence_root).validate_override_inventory(errors)
    return errors


def validate_hotfix_runs(
    acceptance: dict,
    manifest: dict,
    spec: HotfixSpec,
    roster: TesterRoster,
    evidence_store: EvidenceStore,
    prerelease_published_at: datetime,
    now: datetime,
    errors: list[str],
) -> None:
    targets = manifest_targets(manifest, errors)
    required = {
        (board, surface)
        for board in spec.physical_boards
        for surface in spec.surfaces
    }
    seen: set[tuple[str, str]] = set()
    runs = acceptance.get("runs")
    if not isinstance(runs, list):
        errors.append("hotfix runs must be an array")
        return
    for index, run in enumerate(runs):
        label = f"runs[{index}]"
        if not isinstance(run, dict):
            errors.append(f"{label} must be an object")
            continue
        reject_unknown_fields(run, HOTFIX_RUN_FIELDS, label, errors)
        board = run.get("board")
        surface = run.get("surface")
        os_name = run.get("os")
        architecture = run.get("architecture")
        if not all(
            isinstance(value, str)
            for value in (board, surface, os_name, architecture)
        ):
            errors.append(f"{label} board, surface, OS, and architecture must be strings")
            continue
        key = (board, surface)
        if key not in required:
            errors.append(f"{label} is outside the committed hotfix scope")
            continue
        if key in seen:
            errors.append(f"duplicate hotfix qualification result for {key}")
        seen.add(key)
        assignment = roster.physical.get(key)
        if assignment is not None and (os_name, architecture) != (
            assignment.os_name,
            assignment.architecture,
        ):
            errors.append(f"{label} host differs from the signed base roster assignment")
        validate_assignment(run, assignment, label, errors)
        if run.get("result") != "pass":
            errors.append(f"{label} is not a passing hotfix qualification run")
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
        validate_completed_at(run, label, prerelease_published_at, now, errors)
        validate_evidence(run.get("evidence"), label, evidence_store, errors)
        target = targets.get(board, {})
        if run.get("hardware_model") != target.get("display_name"):
            errors.append(f"{label} hardware_model differs from the signed manifest")
        expected_client = "prns-web-flasher" if surface == "web" else "hopspot-flash"
        validate_client(run.get("client"), expected_client, spec.version, label, errors)
        if surface == "web":
            expected_browser = (
                assignment.browser_name
                if assignment is not None and assignment.browser_name is not None
                else "unsupported-browser"
            )
            validate_browser(run.get("browser"), expected_browser, label, errors)
        elif "browser" in run:
            errors.append(f"{label} CLI run must not claim browser evidence")
        scenarios = run.get("scenarios")
        observed = validate_scenarios(
            scenarios, set(spec.required_scenarios), f"{label}.scenarios", errors
        )
        if observed != set(spec.required_scenarios):
            errors.append(f"{label} does not prove the committed hotfix scenarios")
        checks = run.get("checks")
        observed_checks = validate_scenarios(
            checks, set(spec.required_checks), f"{label}.checks", errors
        )
        if observed_checks != set(spec.required_checks):
            errors.append(f"{label} does not prove the committed hotfix checks")
    missing = sorted(required - seen)
    if missing:
        errors.append(f"missing physical hotfix runs: {missing}")


def validate_hotfix_deferrals(
    acceptance: dict,
    spec: HotfixSpec,
    release_owner: object,
    prerelease_published_at: datetime,
    now: datetime,
    errors: list[str],
) -> None:
    entries = acceptance.get("hardware_deferrals")
    if not isinstance(entries, list):
        errors.append("hotfix hardware_deferrals must be an array")
        return
    expected = {entry.board: entry for entry in spec.deferred_hardware}
    seen: set[str] = set()
    for index, entry in enumerate(entries):
        label = f"hardware_deferrals[{index}]"
        if not isinstance(entry, dict):
            errors.append(f"{label} must be an object")
            continue
        reject_unknown_fields(entry, HOTFIX_DEFERRAL_FIELDS, label, errors)
        board = entry.get("board")
        if not isinstance(board, str) or board not in expected:
            errors.append(f"{label} is outside the committed hardware-deferral scope")
            continue
        if board in seen:
            errors.append(f"duplicate hotfix hardware deferral for {board}")
        seen.add(board)
        committed = expected[board]
        if entry.get("basis") != committed.basis:
            errors.append(f"{label} basis differs from the committed hotfix specification")
        if entry.get("follow_up") != committed.follow_up:
            errors.append(f"{label} follow_up differs from the committed hotfix specification")
        if entry.get("approved_by") != release_owner:
            errors.append(f"{label} approved_by must be the signed base-roster release owner")
        validate_completed_at(
            {"completed_at": entry.get("approved_at")},
            label,
            prerelease_published_at,
            now,
            errors,
        )
    missing = sorted(set(expected) - seen)
    if missing:
        errors.append(f"missing committed hardware deferrals: {missing}")


def validate_hotfix_acceptance(
    acceptance: dict,
    manifest: dict,
    arguments: argparse.Namespace,
    raw_roster: object,
    roster: TesterRoster,
    spec: HotfixSpec,
    prerelease_published_at: datetime,
    now: datetime,
) -> list[str]:
    errors: list[str] = []
    reject_unknown_fields(acceptance, HOTFIX_TOP_LEVEL_FIELDS, "acceptance", errors)
    version, _ = validate_candidate_identity(
        acceptance,
        manifest,
        arguments.manifest,
        arguments.manifest_signature,
        arguments.signed_bundle,
        arguments.prerelease_published_at,
        errors,
    )
    if version != spec.version:
        errors.append("hotfix acceptance version differs from its committed specification")
    hotfix = acceptance.get("hotfix")
    expected_hotfix = {
        "version": spec.version,
        "base_version": spec.base_version,
        "base_source_commit": spec.base_source_commit,
        "base_manifest_sha256": spec.base_manifest_sha256,
        "base_release_record_sha256": spec.base_release_record_sha256,
        "base_signed_candidate_sha256": spec.base_signed_candidate_sha256,
        "changed_boards": list(spec.changed_boards),
        "physical_boards": list(spec.physical_boards),
        "deferred_hardware": [
            deferral.document() for deferral in spec.deferred_hardware
        ],
        "summary": spec.summary,
    }
    if not isinstance(hotfix, dict):
        errors.append("hotfix identity must be an object")
    else:
        reject_unknown_fields(hotfix, HOTFIX_IDENTITY_FIELDS, "hotfix", errors)
        if hotfix != expected_hotfix:
            errors.append("acceptance hotfix identity differs from its committed specification")
    release_owner = raw_roster.get("release_owner") if isinstance(raw_roster, dict) else None
    if not is_evidence_text(release_owner):
        errors.append("signed base roster has no release owner")
    evidence_store = EvidenceStore(arguments.evidence_root)
    validate_hotfix_runs(
        acceptance,
        manifest,
        spec,
        roster,
        evidence_store,
        prerelease_published_at,
        now,
        errors,
    )
    validate_hotfix_deferrals(
        acceptance,
        spec,
        release_owner,
        prerelease_published_at,
        now,
        errors,
    )
    evidence_store.validate_inventory(errors)
    return errors


def validate(arguments: argparse.Namespace, now: datetime | None = None) -> list[str]:
    errors: list[str] = []
    acceptance = json.loads(arguments.acceptance.read_text(encoding="utf-8"))
    manifest = json.loads(arguments.manifest.read_text(encoding="utf-8"))
    roster = json.loads(arguments.tester_roster.read_text(encoding="utf-8"))
    if not isinstance(acceptance, dict):
        return ["acceptance document must be a JSON object"]
    if not isinstance(manifest, dict):
        return ["candidate manifest must be a JSON object"]
    try:
        require_schema(manifest)
    except ValueError as error:
        return [str(error)]
    try:
        published_at = parse_utc_timestamp(
            arguments.prerelease_published_at, "prerelease publishedAt"
        )
    except ValueError as error:
        return [str(error)]
    current = now or datetime.now(timezone.utc)
    if current.tzinfo is None:
        return ["acceptance validator current time must include a timezone"]
    current = current.astimezone(timezone.utc)
    version_value = manifest.get("release")
    version = version_value.get("version") if isinstance(version_value, dict) else ""
    hotfix_spec: HotfixSpec | None = None
    if acceptance.get("schema") == HOTFIX_ACCEPTANCE_SCHEMA:
        try:
            hotfix_spec = verify_hotfix_candidate(
                Path(__file__).resolve().parents[2],
                arguments.manifest.resolve().parent,
            )
        except ValueError as error:
            errors.append(str(error))
        if hotfix_spec is None:
            errors.append("schema-6 acceptance requires a target-scoped hotfix candidate")
    roster_version = hotfix_spec.roster_version if hotfix_spec is not None else str(version)
    tester_roster, roster_errors = validate_roster(roster, roster_version)
    errors.extend(f"signed tester roster: {error}" for error in roster_errors)
    if hotfix_spec is not None:
        errors.extend(
            validate_hotfix_acceptance(
                acceptance,
                manifest,
                arguments,
                roster,
                tester_roster,
                hotfix_spec,
                published_at,
                current,
            )
        )
        return errors
    if acceptance.get("schema") == MAINTAINER_OVERRIDE_SCHEMA:
        errors.extend(
            validate_maintainer_override(
                acceptance, roster, manifest, arguments, published_at, current
            )
        )
        return errors
    evidence_store = EvidenceStore(arguments.evidence_root)
    reject_unknown_fields(acceptance, TOP_LEVEL_FIELDS, "acceptance", errors)
    if acceptance.get("schema") != ACCEPTANCE_SCHEMA:
        errors.append(f"acceptance schema must be {ACCEPTANCE_SCHEMA}")
    version, targets = validate_candidate_identity(
        acceptance,
        manifest,
        arguments.manifest,
        arguments.manifest_signature,
        arguments.signed_bundle,
        arguments.prerelease_published_at,
        errors,
    )
    validate_runs(
        acceptance,
        targets,
        version,
        tester_roster,
        evidence_store,
        published_at,
        current,
        errors,
    )
    validate_web_serial_smokes(
        acceptance,
        targets,
        version,
        tester_roster,
        evidence_store,
        published_at,
        current,
        errors,
    )
    validate_fallbacks(
        acceptance,
        version,
        tester_roster,
        evidence_store,
        published_at,
        current,
        errors,
    )
    validate_installation_smokes(
        acceptance,
        version,
        tester_roster,
        evidence_store,
        published_at,
        current,
        errors,
    )
    evidence_store.validate_inventory(errors)
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--acceptance", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--manifest-signature", type=Path, required=True)
    parser.add_argument("--signed-bundle", type=Path, required=True)
    parser.add_argument("--tester-roster", type=Path, required=True)
    parser.add_argument("--evidence-root", type=Path, required=True)
    parser.add_argument("--prerelease-published-at", required=True)
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
    document = json.loads(arguments.acceptance.read_text(encoding="utf-8"))
    if isinstance(document, dict) and document.get("schema") == MAINTAINER_OVERRIDE_SCHEMA:
        print("version-bound maintainer override is bound to the exact signed candidate")
    elif isinstance(document, dict) and document.get("schema") == HOTFIX_ACCEPTANCE_SCHEMA:
        print(
            "target-scoped hotfix acceptance is complete for physical targets and explicit hardware deferrals"
        )
    else:
        print("physical flasher acceptance matrix is complete for the exact signed candidate")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
