#!/usr/bin/env python3
"""Fail closed when prose restates an acceptance count the contract no longer derives.

`flasher_acceptance_contract.py` owns the qualification matrix. Several release documents restate
its sizes in prose so a reader knows what they owe without reading Python. Those restatements do
not update themselves: the acceptance README carried the four-board era's "eight rows" long after
the shipping set grew to five boards, and `release/flash/README.md` carried the same stale number
in a second place while contradicting itself two paragraphs earlier.

This check never stores a count. It derives every number from the contract, then requires each
governed document to state exactly that number. Growing `SHIPPING_BOARDS` turns every affected
sentence red at once, with the file and line to fix.

The sweep below then rejects any *unregistered* count in the same documents, so a new restatement
cannot be added without being bound here too. It recognizes the subjects these documents actually
use; wholly new phrasing for a new quantity still needs a new entry.
"""

from __future__ import annotations

from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[2]
RELEASE_TOOLS = ROOT / "tools" / "release"
if str(RELEASE_TOOLS) not in sys.path:
    sys.path.insert(0, str(RELEASE_TOOLS))

from flasher_acceptance_contract import (  # noqa: E402
    CLI_TARGETS,
    FALLBACK_SCENARIOS,
    OS_ARCHITECTURES,
    REQUIRED_FALLBACKS,
    SHIPPING_BOARDS,
    SURFACES,
)

DERIVED = {
    "physical": len(SHIPPING_BOARDS) * len(SURFACES),
    "boards": len(SHIPPING_BOARDS),
    "fallback": len(REQUIRED_FALLBACKS),
    "fallback_scenarios": len(FALLBACK_SCENARIOS),
    "installer": len(CLI_TARGETS),
    "installer_roster": len(OS_ARCHITECTURES),
}

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

# Every prose restatement of a derived count, as it must read once the numbers are substituted.
GOVERNED = (
    (
        "release/acceptance/README.md",
        "produces {physical} physical rows, {fallback} unsupported-browser rows, and"
        " {installer} native installer rows",
    ),
    (
        "release/acceptance/README.md",
        "(`web` or `cli`): {physical} rows.",
    ),
    (
        "release/acceptance/README.md",
        "Every row must prove all {fallback_scenarios} points:",
    ),
    (
        "release/acceptance/rosters/README.md",
        "It must contain {physical} physical board/surface assignments, {fallback}"
        " Firefox/Safari fallback assignments, and {installer_roster} published-archive"
        " installer assignments.",
    ),
    (
        "release/acceptance/QUALIFICATION.md",
        "The {installer} native installation rows are separate archive checks.",
    ),
    (
        "release/acceptance/QUALIFICATION.md",
        "Its {installer} target-matched jobs re-fetch the public assets",
    ),
    (
        "release/flash/README.md",
        "assign the {physical} physical, {fallback} fallback, and {installer_roster}"
        " archive-installation coverage slots",
    ),
    (
        "release/flash/README.md",
        "validates {physical} full transport-aware physical rows, {fallback} browser"
        " fallbacks, and all {installer} installer/exact-version smokes;",
    ),
    (
        "docs/release-dependency-audit.md",
        "the complete web/CLI {boards}-board matrix",
    ),
)

# Subjects that make a nearby number a qualification count rather than ordinary prose.
SWEEP_SUBJECTS = (
    r"physical",
    r"rows?\b",
    r"(?:browser|Firefox/Safari)?\s*fallbacks?\b",
    r"unsupported-browser rows?",
    r"native install(?:er|ation) rows?",
    r"installer",
    r"board matrix",
    r"board/surface assignments",
    r"target-matched jobs",
    r"shipping boards?",
    r"points:",
)
SWEEP = re.compile(
    r"\b(?:" + "|".join(NUMBER_WORDS) + r")[\s-]+(?:" + "|".join(SWEEP_SUBJECTS) + r")",
    re.IGNORECASE,
)


def spell(count: int) -> str:
    """Render a derived count the way these documents write it, or fail rather than guess."""
    if not 0 <= count < len(NUMBER_WORDS):
        raise SystemExit(
            f"acceptance doc contract: derived count {count} has no spelling; extend NUMBER_WORDS"
        )
    return NUMBER_WORDS[count]


def flatten(text: str) -> tuple[str, list[int]]:
    """Join the document into single-spaced text so a sentence that soft-wraps still matches.

    Returns the flattened text and, for each of its characters, the source line it came from.
    """
    parts: list[str] = []
    lines: list[int] = []
    for number, raw in enumerate(text.splitlines(), start=1):
        stripped = raw.strip()
        if not stripped:
            continue
        if parts:
            parts.append(" ")
            lines.append(number)
        parts.append(stripped)
        lines.extend([number] * len(stripped))
    return "".join(parts), lines


def check() -> list[str]:
    errors: list[str] = []
    spelled = {key: spell(count) for key, count in DERIVED.items()}
    documents = {path for path, _ in GOVERNED}
    flattened: dict[str, tuple[str, list[int]]] = {}
    for path in sorted(documents):
        try:
            flattened[path] = flatten((ROOT / path).read_text(encoding="utf-8"))
        except OSError as error:
            errors.append(f"{path}: cannot read governed document: {error}")
    if errors:
        return errors

    covered: dict[str, list[tuple[int, int]]] = {path: [] for path in documents}
    unbound: set[str] = set()
    for path, template in GOVERNED:
        expected = template.format(**spelled)
        text, _ = flattened[path]
        found = text.find(expected)
        if found < 0:
            errors.append(
                f"{path}: expected the contract-derived sentence \"{expected}\".\n"
                f"    The counts come from tools/release/flasher_acceptance_contract.py."
                f" Update the document, or bind the new wording in this check."
            )
            unbound.add(path)
            continue
        covered[path].append((found, found + len(expected)))
        duplicate = text.find(expected, found + len(expected))
        if duplicate >= 0:
            _, lines = flattened[path]
            errors.append(
                f"{path}:{lines[duplicate]}: this sentence is stated twice;"
                f" a second copy is a second thing to forget"
            )

    for path in sorted(documents):
        # A document with an unmatched sentence already reported the real defect; sweeping its
        # leftovers would only bury that message under fragments of the same line.
        if path in unbound:
            continue
        text, lines = flattened[path]
        spans = covered[path]
        for match in SWEEP.finditer(text):
            if any(start <= match.start() < end for start, end in spans):
                continue
            errors.append(
                f"{path}:{lines[match.start()]}: \"{match.group().strip()}\" restates a"
                f" qualification count that nothing derives.\n"
                f"    Bind it in validation/release/acceptance-doc-contracts.py so it cannot"
                f" go stale, or reword it to not carry a number."
            )
    return errors


def main() -> int:
    errors = check()
    for error in errors:
        print(f"acceptance doc contract: {error}", file=sys.stderr)
    if errors:
        return 1
    print(
        f"acceptance documents state the derived {DERIVED['physical']} physical,"
        f" {DERIVED['fallback']} fallback, and {DERIVED['installer']} installer counts"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
