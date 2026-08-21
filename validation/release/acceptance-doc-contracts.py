#!/usr/bin/env python3

from __future__ import annotations

import json
from pathlib import Path
import re
import sys
from typing import Mapping, NamedTuple


ROOT = Path(__file__).resolve().parents[2]
RELEASE_TOOLS = ROOT / "tools" / "release"
if str(RELEASE_TOOLS) not in sys.path:
    sys.path.insert(0, str(RELEASE_TOOLS))

from flasher_acceptance_contract import (  # noqa: E402
    CLI_TARGETS,
    FALLBACK_SCENARIOS,
    REQUIRED_FALLBACKS,
    SHIPPING_BOARDS,
    SURFACES,
    UF2_COMPATIBILITY_VARIANTS,
    WEB_SERIAL_HOSTS,
    WEB_SERIAL_SCENARIOS,
)


class CountContract(NamedTuple):
    path: str
    before: str
    derived: str
    after: str


class FlattenedDocument(NamedTuple):
    text: str
    source_lines: tuple[int, ...]


NUMBER_WORDS = (
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
)
COUNT_TOKEN = r"(?P<count>[0-9]+|(?i:" + "|".join(NUMBER_WORDS) + r"))"

COUNT_CONTRACTS = (
    CountContract(
        "release/acceptance/README.md",
        "produces ",
        "physical",
        " physical rows",
    ),
    CountContract(
        "release/acceptance/README.md",
        "physical rows, ",
        "web_serial",
        " Firefox Web Serial rows",
    ),
    CountContract(
        "release/acceptance/README.md",
        "Firefox Web Serial rows, ",
        "fallback",
        " unsupported-browser row",
    ),
    CountContract(
        "release/acceptance/README.md",
        "unsupported-browser row, and ",
        "installer",
        " native installer rows",
    ),
    CountContract(
        "release/acceptance/README.md",
        "both surfaces: ",
        "physical",
        " rows.",
    ),
    CountContract(
        "release/acceptance/README.md",
        "Every row must prove all ",
        "fallback_scenarios",
        " points:",
    ),
    CountContract(
        "release/acceptance/README.md",
        "must pass exactly ",
        "web_serial_scenarios",
        " scenarios:",
    ),
    CountContract(
        "release/acceptance/rosters/README.md",
        "It must contain ",
        "physical_assignments",
        " physical board/surface assignments",
    ),
    CountContract(
        "release/acceptance/rosters/README.md",
        "physical board/surface assignments, ",
        "web_serial_roster",
        " Firefox Web Serial assignments",
    ),
    CountContract(
        "release/acceptance/rosters/README.md",
        "Firefox Web Serial assignments, ",
        "fallback",
        " Safari fallback assignment",
    ),
    CountContract(
        "release/acceptance/rosters/README.md",
        "Safari fallback assignment, and ",
        "installer_roster",
        " published-archive installer assignments.",
    ),
    CountContract(
        "release/acceptance/QUALIFICATION.md",
        "The ",
        "installer",
        " native installation rows are separate archive checks.",
    ),
    CountContract(
        "release/acceptance/QUALIFICATION.md",
        "Its ",
        "installer",
        " target-matched jobs re-fetch the public assets",
    ),
    CountContract(
        "release/flash/README.md",
        "assign the ",
        "physical_assignments",
        " physical",
    ),
    CountContract(
        "release/flash/README.md",
        "physical, ",
        "web_serial_roster",
        " Firefox Web Serial",
    ),
    CountContract(
        "release/flash/README.md",
        "Firefox Web Serial, ",
        "fallback",
        " Safari fallback",
    ),
    CountContract(
        "release/flash/README.md",
        "fallback, and ",
        "installer_roster",
        " archive-installation coverage slots",
    ),
    CountContract(
        "release/flash/README.md",
        "validates ",
        "physical",
        " full transport-aware physical rows",
    ),
    CountContract(
        "release/flash/README.md",
        "physical rows, ",
        "web_serial",
        " Firefox Web Serial smokes",
    ),
    CountContract(
        "release/flash/README.md",
        "Firefox Web Serial smokes, ",
        "fallback",
        " Safari fallback",
    ),
    CountContract(
        "release/flash/README.md",
        "Safari fallback, and all ",
        "installer",
        " installer/exact-version smokes",
    ),
)


def physical_row_count(root: Path) -> int:
    boards = json.loads((root / "release" / "flash" / "boards.json").read_text(encoding="utf-8"))
    transports = {
        board["slug"]: board["transport"]
        for board in boards["boards"]
        if board["availability"] == "shipping"
    }
    if set(transports) != set(SHIPPING_BOARDS):
        raise ValueError(
            "release/flash/boards.json does not state exactly the shipping board set"
        )
    return sum(
        len(SURFACES)
        * (len(UF2_COMPATIBILITY_VARIANTS[board]) if board in UF2_COMPATIBILITY_VARIANTS else 1)
        for board in SHIPPING_BOARDS
    )


def derived_counts(root: Path) -> dict[str, int]:
    return {
        "physical": physical_row_count(root),
        "physical_assignments": len(SHIPPING_BOARDS) * len(SURFACES),
        "boards": len(SHIPPING_BOARDS),
        "fallback": len(REQUIRED_FALLBACKS),
        "fallback_scenarios": len(FALLBACK_SCENARIOS),
        "web_serial": len(WEB_SERIAL_HOSTS),
        "web_serial_roster": len(WEB_SERIAL_HOSTS),
        "web_serial_scenarios": len(WEB_SERIAL_SCENARIOS),
        "installer": len(CLI_TARGETS),
        "installer_roster": len(CLI_TARGETS),
    }


def parse_count(token: str) -> int:
    if token.isdecimal():
        return int(token)
    return NUMBER_WORDS.index(token.lower())


def flatten(text: str) -> FlattenedDocument:
    parts: list[str] = []
    source_lines: list[int] = []
    for line_number, raw in enumerate(text.splitlines(), start=1):
        stripped = raw.strip()
        if not stripped:
            continue
        if parts:
            parts.append(" ")
            source_lines.append(line_number)
        parts.append(stripped)
        source_lines.extend([line_number] * len(stripped))
    return FlattenedDocument("".join(parts), tuple(source_lines))


def check(
    root: Path,
    derived: Mapping[str, int],
    contracts: tuple[CountContract, ...],
) -> list[str]:
    errors: list[str] = []
    documents: dict[str, FlattenedDocument] = {}
    for path in sorted({contract.path for contract in contracts}):
        try:
            documents[path] = flatten((root / path).read_text(encoding="utf-8"))
        except OSError as error:
            errors.append(f"{path}: cannot read governed document: {error}")
    if errors:
        return errors

    for contract in contracts:
        document = documents[contract.path]
        pattern = re.compile(
            re.escape(contract.before) + COUNT_TOKEN + re.escape(contract.after)
        )
        matches = list(pattern.finditer(document.text))
        if not matches:
            errors.append(
                f"{contract.path}: cannot find the registered {contract.derived} count between "
                f"{contract.before!r} and {contract.after!r}"
            )
            continue
        if len(matches) > 1:
            duplicate = matches[1]
            errors.append(
                f"{contract.path}:{document.source_lines[duplicate.start('count')]}: "
                f"the registered {contract.derived} count occurs more than once"
            )
            continue
        match = matches[0]
        observed_token = match.group("count")
        observed = parse_count(observed_token)
        expected = derived[contract.derived]
        if observed != expected:
            errors.append(
                f"{contract.path}:{document.source_lines[match.start('count')]}: "
                f"expected derived {contract.derived} count {expected}, found {observed_token}"
            )
    return errors


def main() -> int:
    try:
        derived = derived_counts(ROOT)
    except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as error:
        print(f"acceptance doc contract: cannot derive counts: {error}", file=sys.stderr)
        return 1
    errors = check(ROOT, derived, COUNT_CONTRACTS)
    for error in errors:
        print(f"acceptance doc contract: {error}", file=sys.stderr)
    if errors:
        return 1
    print(
        f"acceptance documents state the derived {derived['physical']} physical,"
        f" {derived['web_serial']} Firefox Web Serial, {derived['fallback']} fallback,"
        f" and {derived['installer']} installer counts"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
