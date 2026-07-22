"""Validated tester assignments for one immutable flasher candidate."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date

from flasher_acceptance_contract import CLI_TARGETS, SHIPPING_BOARDS


TOP_LEVEL_FIELDS = {"schema", "release", "release_owner", "confirmed_on", "assignments"}
RELEASE_FIELDS = {"version"}
ASSIGNMENT_FIELDS = {
    "os",
    "architecture",
    "cli_target",
    "web_browser",
    "tester",
    "boards",
    "cables_ready",
    "device_permissions_ready",
    "recovery_instructions_reviewed",
}
BROWSER_FIELDS = {"name", "channel"}
REQUIRED_HOSTS = set(CLI_TARGETS.values())
PLACEHOLDERS = ("REPLACE", "TODO", "TBD", "UNKNOWN", "NOT_RUN", "NOT-RUN", "UNASSIGNED")


@dataclass(frozen=True)
class TesterAssignment:
    tester: str
    cli_target: str
    browser_name: str


def reject_unknown(record: dict, allowed: set[str], label: str, errors: list[str]) -> None:
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


def validate_roster(
    roster: object,
    expected_version: str,
) -> tuple[dict[tuple[str, str], TesterAssignment], list[str]]:
    errors: list[str] = []
    if not isinstance(roster, dict):
        return {}, ["tester roster must be a JSON object"]
    reject_unknown(roster, TOP_LEVEL_FIELDS, "roster", errors)
    if roster.get("schema") != 1:
        errors.append("tester roster schema must be 1")
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

    assignments = roster.get("assignments")
    if not isinstance(assignments, list):
        errors.append("tester roster assignments must be an array")
        return {}, errors
    by_host: dict[tuple[str, str], TesterAssignment] = {}
    for index, assignment in enumerate(assignments):
        label = f"assignments[{index}]"
        if not isinstance(assignment, dict):
            errors.append(f"{label} must be an object")
            continue
        reject_unknown(assignment, ASSIGNMENT_FIELDS, label, errors)
        os_name = assignment.get("os")
        architecture = assignment.get("architecture")
        host = (os_name, architecture)
        if not isinstance(os_name, str) or not isinstance(architecture, str):
            errors.append(f"{label} OS and architecture must be strings")
            continue
        if host not in REQUIRED_HOSTS:
            errors.append(f"{label} is not a published host architecture")
        elif host in by_host:
            errors.append(f"duplicate tester assignment for {host}")

        cli_target = assignment.get("cli_target")
        expected_target = next(
            (target for target, target_host in CLI_TARGETS.items() if target_host == host),
            None,
        )
        if cli_target != expected_target:
            errors.append(f"{label} cli_target does not match its host architecture")
        browser = assignment.get("web_browser")
        expected_browser = "edge" if os_name == "windows" else "chrome"
        if not isinstance(browser, dict):
            errors.append(f"{label} web_browser must be an object")
        else:
            reject_unknown(browser, BROWSER_FIELDS, f"{label}.web_browser", errors)
            if browser != {"name": expected_browser, "channel": "stable"}:
                errors.append(
                    f"{label} web_browser must be stable {expected_browser} for this host"
                )
        tester = assignment.get("tester")
        if not real_identity(tester):
            errors.append(f"{label} must name a nonsecret tester identity")
        boards = assignment.get("boards")
        if (
            not isinstance(boards, list)
            or not all(isinstance(board, str) for board in boards)
            or len(boards) != len(SHIPPING_BOARDS)
            or len(set(boards)) != len(boards)
            or set(boards) != set(SHIPPING_BOARDS)
        ):
            errors.append(f"{label} must confirm access to all four shipping boards")
        readiness = (
            "cables_ready",
            "device_permissions_ready",
            "recovery_instructions_reviewed",
        )
        incomplete = sorted(field for field in readiness if assignment.get(field) is not True)
        if incomplete:
            errors.append(f"{label} readiness is incomplete: {incomplete}")

        if (
            host in REQUIRED_HOSTS
            and host not in by_host
            and isinstance(tester, str)
            and isinstance(cli_target, str)
            and isinstance(browser, dict)
            and isinstance(browser.get("name"), str)
        ):
            by_host[host] = TesterAssignment(
                tester=tester,
                cli_target=cli_target,
                browser_name=browser["name"],
            )

    missing = sorted(REQUIRED_HOSTS - set(by_host))
    if missing:
        errors.append(f"tester roster is missing host architectures: {missing}")
    return by_host, errors
