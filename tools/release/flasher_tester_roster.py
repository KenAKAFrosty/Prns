from __future__ import annotations

from dataclasses import dataclass
from datetime import date

from flasher_acceptance_contract import (
    CLI_TARGETS,
    OS_ARCHITECTURES,
    REQUIRED_FALLBACKS,
    SHIPPING_BOARDS,
    SURFACES,
)


TOP_LEVEL_FIELDS = {
    "schema",
    "release",
    "release_owner",
    "confirmed_on",
    "physical_assignments",
    "fallback_assignments",
    "installation_assignments",
}
RELEASE_FIELDS = {"version"}
PHYSICAL_FIELDS = {
    "board",
    "surface",
    "os",
    "architecture",
    "browser",
    "tester",
    "cables_ready",
    "device_permissions_ready",
    "recovery_instructions_reviewed",
}
FALLBACK_FIELDS = {
    "browser",
    "os",
    "architecture",
    "tester",
    "browser_ready",
}
INSTALLATION_FIELDS = {
    "target",
    "os",
    "architecture",
    "tester",
    "archive_ready",
}
BROWSER_FIELDS = {"name", "channel"}
PLACEHOLDERS = (
    "REPLACE",
    "TODO",
    "TBD",
    "UNKNOWN",
    "NOT_RUN",
    "NOT-RUN",
    "UNASSIGNED",
)


@dataclass(frozen=True)
class PhysicalAssignment:
    tester: str
    board: str
    surface: str
    os_name: str
    architecture: str
    browser_name: str | None


@dataclass(frozen=True)
class FallbackAssignment:
    tester: str
    browser_name: str
    os_name: str
    architecture: str


@dataclass(frozen=True)
class InstallationAssignment:
    tester: str
    target: str
    os_name: str
    architecture: str


@dataclass(frozen=True)
class TesterRoster:
    physical: dict[tuple[str, str], PhysicalAssignment]
    fallbacks: dict[tuple[str, str], FallbackAssignment]
    installations: dict[str, InstallationAssignment]


def reject_unknown(
    record: dict,
    allowed: set[str],
    label: str,
    errors: list[str],
) -> None:
    unknown = sorted(set(record) - allowed)
    if unknown:
        errors.append(f"{label} contains unknown fields: {unknown}")


def real_identity(value: object) -> bool:
    return (
        isinstance(value, str)
        and value == value.strip()
        and 1 <= len(value) <= 80
        and not value.upper().startswith(PLACEHOLDERS)
        and not any(ord(character) < 0x20 for character in value)
        and " " not in value
        and not ("@" in value and "." in value.split("@", 1)[-1])
    )


def validate_date(value: object, errors: list[str]) -> None:
    if not isinstance(value, str):
        errors.append("roster confirmed_on must be ISO YYYY-MM-DD")
        return
    try:
        confirmed = date.fromisoformat(value)
    except ValueError:
        errors.append("roster confirmed_on must be ISO YYYY-MM-DD")
        return
    if confirmed > date.today():
        errors.append("roster confirmed_on cannot be in the future")


def validate_browser(
    value: object,
    expected_name: str,
    label: str,
    errors: list[str],
) -> str | None:
    if not isinstance(value, dict):
        errors.append(f"{label} browser must be an object")
        return None
    reject_unknown(value, BROWSER_FIELDS, f"{label}.browser", errors)
    if value != {"name": expected_name, "channel": "stable"}:
        errors.append(f"{label} browser must be stable {expected_name}")
        return None
    return expected_name


def validate_physical_assignments(
    value: object,
    errors: list[str],
) -> dict[tuple[str, str], PhysicalAssignment]:
    if not isinstance(value, list):
        errors.append("tester roster physical_assignments must be an array")
        return {}
    assignments: dict[tuple[str, str], PhysicalAssignment] = {}
    os_coverage = {surface: set() for surface in SURFACES}
    for index, assignment in enumerate(value):
        label = f"physical_assignments[{index}]"
        if not isinstance(assignment, dict):
            errors.append(f"{label} must be an object")
            continue
        reject_unknown(assignment, PHYSICAL_FIELDS, label, errors)
        board = assignment.get("board")
        surface = assignment.get("surface")
        os_name = assignment.get("os")
        architecture = assignment.get("architecture")
        tester = assignment.get("tester")
        if not all(
            isinstance(item, str)
            for item in (board, surface, os_name, architecture)
        ):
            errors.append(f"{label} board, surface, OS, and architecture must be strings")
            continue
        key = (board, surface)
        if board not in SHIPPING_BOARDS or surface not in SURFACES:
            errors.append(f"{label} is not a shipping board/surface assignment")
        elif key in assignments:
            errors.append(f"duplicate physical assignment for {key}")
        if (os_name, architecture) not in OS_ARCHITECTURES:
            errors.append(f"{label} is not a supported host architecture")
        if not real_identity(tester):
            errors.append(f"{label} must name a nonsecret tester identity")
        browser_name = None
        if surface == "web":
            expected_browser = "edge" if os_name == "windows" else "chrome"
            browser_name = validate_browser(
                assignment.get("browser"),
                expected_browser,
                label,
                errors,
            )
        elif "browser" in assignment:
            errors.append(f"{label} CLI assignment must not contain a browser")
        readiness = (
            "cables_ready",
            "device_permissions_ready",
            "recovery_instructions_reviewed",
        )
        incomplete = sorted(
            field for field in readiness if assignment.get(field) is not True
        )
        if incomplete:
            errors.append(f"{label} readiness is incomplete: {incomplete}")
        if (
            board in SHIPPING_BOARDS
            and surface in SURFACES
            and key not in assignments
            and (os_name, architecture) in OS_ARCHITECTURES
            and isinstance(tester, str)
        ):
            assignments[key] = PhysicalAssignment(
                tester=tester,
                board=board,
                surface=surface,
                os_name=os_name,
                architecture=architecture,
                browser_name=browser_name,
            )
            os_coverage[surface].add(os_name)
    required = {
        (board, surface) for board in SHIPPING_BOARDS for surface in SURFACES
    }
    missing = sorted(required - set(assignments))
    if missing:
        errors.append(f"tester roster is missing physical assignments: {missing}")
    for surface in SURFACES:
        missing_oses = sorted({"linux", "macos", "windows"} - os_coverage[surface])
        if missing_oses:
            errors.append(
                f"{surface} physical assignments do not cover host OSes: {missing_oses}"
            )
    if len(value) != len(required):
        errors.append("tester roster must contain exactly eight physical assignments")
    return assignments


def validate_fallback_assignments(
    value: object,
    errors: list[str],
) -> dict[tuple[str, str], FallbackAssignment]:
    if not isinstance(value, list):
        errors.append("tester roster fallback_assignments must be an array")
        return {}
    assignments: dict[tuple[str, str], FallbackAssignment] = {}
    for index, assignment in enumerate(value):
        label = f"fallback_assignments[{index}]"
        if not isinstance(assignment, dict):
            errors.append(f"{label} must be an object")
            continue
        reject_unknown(assignment, FALLBACK_FIELDS, label, errors)
        browser = assignment.get("browser")
        os_name = assignment.get("os")
        architecture = assignment.get("architecture")
        tester = assignment.get("tester")
        browser_name = (
            browser.get("name") if isinstance(browser, dict) else None
        )
        key = (browser_name, os_name)
        if key not in REQUIRED_FALLBACKS:
            errors.append(f"{label} is not a required Firefox/Safari assignment")
        elif key in assignments:
            errors.append(f"duplicate fallback assignment for {key}")
        if isinstance(browser_name, str):
            validate_browser(browser, browser_name, label, errors)
        else:
            errors.append(f"{label} browser must be an object")
        if (os_name, architecture) not in OS_ARCHITECTURES:
            errors.append(f"{label} is not a supported host architecture")
        if not real_identity(tester):
            errors.append(f"{label} must name a nonsecret tester identity")
        if assignment.get("browser_ready") is not True:
            errors.append(f"{label} browser_ready must be true")
        if (
            key in REQUIRED_FALLBACKS
            and key not in assignments
            and isinstance(browser_name, str)
            and isinstance(os_name, str)
            and isinstance(architecture, str)
            and isinstance(tester, str)
        ):
            assignments[key] = FallbackAssignment(
                tester=tester,
                browser_name=browser_name,
                os_name=os_name,
                architecture=architecture,
            )
    missing = sorted(REQUIRED_FALLBACKS - set(assignments))
    if missing:
        errors.append(f"tester roster is missing fallback assignments: {missing}")
    if len(value) != len(REQUIRED_FALLBACKS):
        errors.append("tester roster must contain exactly four fallback assignments")
    return assignments


def validate_installation_assignments(
    value: object,
    errors: list[str],
) -> dict[str, InstallationAssignment]:
    if not isinstance(value, list):
        errors.append("tester roster installation_assignments must be an array")
        return {}
    assignments: dict[str, InstallationAssignment] = {}
    for index, assignment in enumerate(value):
        label = f"installation_assignments[{index}]"
        if not isinstance(assignment, dict):
            errors.append(f"{label} must be an object")
            continue
        reject_unknown(assignment, INSTALLATION_FIELDS, label, errors)
        target = assignment.get("target")
        os_name = assignment.get("os")
        architecture = assignment.get("architecture")
        tester = assignment.get("tester")
        if not isinstance(target, str) or target not in CLI_TARGETS:
            errors.append(f"{label} is not a published CLI target")
            continue
        if target in assignments:
            errors.append(f"duplicate installation assignment for {target}")
        if (os_name, architecture) != CLI_TARGETS[target]:
            errors.append(f"{label} host does not match target {target}")
        if not real_identity(tester):
            errors.append(f"{label} must name a nonsecret tester identity")
        if assignment.get("archive_ready") is not True:
            errors.append(f"{label} archive_ready must be true")
        if (
            target not in assignments
            and isinstance(os_name, str)
            and isinstance(architecture, str)
            and isinstance(tester, str)
        ):
            assignments[target] = InstallationAssignment(
                tester=tester,
                target=target,
                os_name=os_name,
                architecture=architecture,
            )
    missing = sorted(set(CLI_TARGETS) - set(assignments))
    if missing:
        errors.append(f"tester roster is missing installation assignments: {missing}")
    if len(value) != len(CLI_TARGETS):
        errors.append("tester roster must contain exactly five installation assignments")
    return assignments


def validate_roster(
    roster: object,
    expected_version: str,
) -> tuple[TesterRoster, list[str]]:
    errors: list[str] = []
    empty = TesterRoster(physical={}, fallbacks={}, installations={})
    if not isinstance(roster, dict):
        return empty, ["tester roster must be a JSON object"]
    reject_unknown(roster, TOP_LEVEL_FIELDS, "roster", errors)
    if roster.get("schema") != 2:
        errors.append("tester roster schema must be 2")
    if not real_identity(roster.get("release_owner")):
        errors.append("tester roster must name a nonsecret release_owner identity")
    validate_date(roster.get("confirmed_on"), errors)
    release = roster.get("release")
    if not isinstance(release, dict):
        errors.append("tester roster release must be an object")
    else:
        reject_unknown(release, RELEASE_FIELDS, "roster.release", errors)
        if release != {"version": expected_version}:
            errors.append("tester roster release identity differs from the candidate")
    physical = validate_physical_assignments(
        roster.get("physical_assignments"),
        errors,
    )
    fallbacks = validate_fallback_assignments(
        roster.get("fallback_assignments"),
        errors,
    )
    installations = validate_installation_assignments(
        roster.get("installation_assignments"),
        errors,
    )
    return (
        TesterRoster(
            physical=physical,
            fallbacks=fallbacks,
            installations=installations,
        ),
        errors,
    )
