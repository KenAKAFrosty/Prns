#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys

from flasher_board_catalog import boards_from_catalog


TEMPLATE_SCHEMA = 4
ROSTER_SCHEMA = 3
SURFACES = frozenset(("cli", "web"))


def create(template: Path, catalog_path: Path, version: str, output: Path) -> None:
    document = json.loads(template.read_text(encoding="utf-8"))
    if not isinstance(document, dict) or document.get("schema") != TEMPLATE_SCHEMA:
        raise ValueError(f"tester roster template schema must be {TEMPLATE_SCHEMA}")
    assignments = document.get("physical_assignments")
    if not isinstance(assignments, list):
        raise ValueError("tester roster template physical_assignments must be an array")
    boards = boards_from_catalog(catalog_path)
    expected = {(board, surface) for board in boards.catalog for surface in SURFACES}
    observed: set[tuple[str, str]] = set()
    for index, assignment in enumerate(assignments):
        if not isinstance(assignment, dict):
            raise ValueError(f"tester roster template physical assignment {index} must be an object")
        board = assignment.get("board")
        surface = assignment.get("surface")
        if not isinstance(board, str) or surface not in SURFACES:
            raise ValueError(f"tester roster template physical assignment {index} is malformed")
        key = (board, surface)
        if key in observed:
            raise ValueError(f"tester roster template duplicates {board}/{surface}")
        observed.add(key)
    if observed != expected:
        raise ValueError("tester roster template does not cover exactly every catalog board and surface")
    shipping = set(boards.shipping)
    document["schema"] = ROSTER_SCHEMA
    document["release"] = {"version": version}
    document["physical_assignments"] = [
        assignment for assignment in assignments if assignment["board"] in shipping
    ]
    output.parent.mkdir(parents=True, exist_ok=True)
    try:
        descriptor = os.open(output, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError as error:
        raise ValueError(f"refusing to overwrite existing tester roster: {output}") from error
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as stream:
            json.dump(document, stream, indent=2)
            stream.write("\n")
    except BaseException:
        output.unlink(missing_ok=True)
        raise


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument(
        "--template",
        type=Path,
        default=root / "release" / "acceptance" / "roster-template.json",
    )
    parser.add_argument(
        "--catalog",
        type=Path,
        default=root / "release" / "flash" / "boards.json",
    )
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    if not arguments.version or arguments.version.lower() == "next":
        parser.error("an immutable candidate version is required")
    try:
        create(arguments.template, arguments.catalog, arguments.version, arguments.output)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"tester roster creation failed: {error}", file=sys.stderr)
        return 1
    print(arguments.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
