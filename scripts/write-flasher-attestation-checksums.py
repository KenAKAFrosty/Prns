#!/usr/bin/env python3
"""Write exact canonical name/digest pairs for actions/attest."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path, PurePosixPath
import sys


def sha256(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def canonical_name(value: str) -> str:
    path = PurePosixPath(value)
    if (
        "\\" in value
        or path.is_absolute()
        or not path.parts
        or any(part in {"", ".", ".."} for part in path.parts)
        or path.as_posix() != value
    ):
        raise ValueError(f"attestation subject name is not canonical: {value!r}")
    return value


def build(subjects: list[list[str]]) -> list[tuple[str, str]]:
    output = []
    names = set()
    paths = set()
    for raw_name, raw_path in subjects:
        name = canonical_name(raw_name)
        path = Path(raw_path)
        if name in names:
            raise ValueError(f"attestation subject name is duplicated: {name}")
        resolved = path.resolve()
        if resolved in paths:
            raise ValueError(f"attestation subject file is repeated: {path}")
        if not path.is_file():
            raise ValueError(f"attestation subject file is unavailable: {path}")
        names.add(name)
        paths.add(resolved)
        output.append((name, sha256(path)))
    if not output:
        raise ValueError("at least one attestation subject is required")
    return sorted(output)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--subject", nargs=2, action="append", required=True, metavar=("NAME", "PATH")
    )
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        subjects = build(arguments.subject)
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        arguments.output.write_text(
            "".join(f"{checksum}  {name}\n" for name, checksum in subjects),
            encoding="utf-8",
            newline="\n",
        )
    except (OSError, ValueError) as error:
        print(f"attestation checksum generation failed: {error}", file=sys.stderr)
        return 1
    print(arguments.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
