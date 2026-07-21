#!/usr/bin/env python3
"""Write deterministic release-candidate provenance without environment secrets."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from flasher_build_metadata import build_metadata, resolved_tools


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--source-date-epoch", type=int, required=True)
    arguments = parser.parse_args()
    try:
        metadata = build_metadata(
            commit=arguments.commit,
            source_date_epoch=arguments.source_date_epoch,
            tools=resolved_tools(),
        )
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        arguments.output.write_text(
            json.dumps(metadata, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
            newline="\n",
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"release build metadata failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
