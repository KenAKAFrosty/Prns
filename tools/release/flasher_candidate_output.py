"""Resolve flasher candidate output once, before release builds change directories."""

from __future__ import annotations

import argparse
from pathlib import Path
import sys


def resolve_output(root: Path, requested: Path, *, cwd: Path | None = None) -> Path:
    repository = root.resolve()
    base = Path.cwd() if cwd is None else cwd
    candidate = requested.resolve() if requested.is_absolute() else (base / requested).resolve()
    try:
        candidate.relative_to(repository)
    except ValueError:
        return candidate

    build_root = repository / "target"
    if candidate == build_root or build_root not in candidate.parents:
        raise ValueError(
            "an in-repository candidate output must be a dedicated directory beneath target/"
        )
    return candidate


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("repository", type=Path)
    parser.add_argument("candidate", type=Path)
    arguments = parser.parse_args()
    try:
        print(resolve_output(arguments.repository, arguments.candidate))
    except ValueError as error:
        print(f"candidate output path rejected: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
