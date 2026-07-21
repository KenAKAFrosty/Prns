#!/usr/bin/env python3
"""Prepare, apply, or validate cumulative signed website release history."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from flasher_website_history import (
    apply_history,
    prepare_bootstrap,
    prepare_retained,
    stable_descriptor_identity,
    validate_candidate_history,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    bootstrap = commands.add_parser("bootstrap")
    bootstrap.add_argument("--output", type=Path, required=True)
    retain = commands.add_parser("retain")
    retain.add_argument("--candidate", type=Path, required=True)
    retain.add_argument("--release-record", type=Path, required=True)
    retain.add_argument("--output", type=Path, required=True)
    apply = commands.add_parser("apply")
    apply.add_argument("--history", type=Path, required=True)
    apply.add_argument("--candidate", type=Path, required=True)
    validate = commands.add_parser("validate-candidate")
    validate.add_argument("--candidate", type=Path, required=True)
    probe = commands.add_parser("probe-stable")
    probe.add_argument("--descriptor", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if arguments.command == "bootstrap":
            result = prepare_bootstrap(arguments.output)
        elif arguments.command == "retain":
            result = prepare_retained(
                arguments.candidate, arguments.release_record, arguments.output
            )
        elif arguments.command == "apply":
            result = apply_history(arguments.history, arguments.candidate)
        elif arguments.command == "probe-stable":
            try:
                identity = stable_descriptor_identity(arguments.descriptor)
            except (OSError, ValueError, json.JSONDecodeError) as error:
                print(f"flasher website history failed: {error}", file=sys.stderr)
                return 2
            if identity is None:
                print(json.dumps({"stable": False}, sort_keys=True))
                return 1
            print(json.dumps({"stable": True, **identity}, sort_keys=True))
            return 0
        else:
            result = validate_candidate_history(arguments.candidate)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"flasher website history failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps({"mode": result["mode"], "tree": result["tree"]}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
