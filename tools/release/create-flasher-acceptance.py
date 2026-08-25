#!/usr/bin/env python3
"""Create a truthful, entirely not-run qualification record for one signed candidate."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys
import tempfile

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from flasher_acceptance_contract import hotfix_scaffold, scaffold  # noqa: E402
from flasher_hotfix import verify_candidate as verify_hotfix_candidate  # noqa: E402
from flasher_tester_roster import validate_roster  # noqa: E402


def create(arguments: argparse.Namespace) -> None:
    for label, path in (
        ("manifest", arguments.manifest),
        ("manifest signature", arguments.manifest_signature),
        ("signed candidate bundle", arguments.signed_bundle),
        ("tester roster", arguments.tester_roster),
    ):
        if not path.is_file():
            raise ValueError(f"{label} is not a file: {path}")

    manifest = json.loads(arguments.manifest.read_text(encoding="utf-8"))
    if not isinstance(manifest, dict):
        raise ValueError("manifest must be a JSON object")
    roster = json.loads(arguments.tester_roster.read_text(encoding="utf-8"))
    release = manifest.get("release")
    version = release.get("version") if isinstance(release, dict) else ""
    hotfix = verify_hotfix_candidate(
        Path(__file__).resolve().parents[2], arguments.manifest.resolve().parent
    )
    roster_version = hotfix.roster_version if hotfix is not None else str(version)
    tester_roster, roster_errors = validate_roster(roster, roster_version)
    if roster_errors:
        raise ValueError(
            "tester roster is invalid: " + "; ".join(roster_errors)
        )
    if hotfix is None:
        record = scaffold(
            manifest,
            arguments.manifest,
            arguments.manifest_signature,
            arguments.signed_bundle,
            arguments.prerelease_published_at,
            tester_roster,
        )
    else:
        record = hotfix_scaffold(
            manifest,
            arguments.manifest,
            arguments.manifest_signature,
            arguments.signed_bundle,
            arguments.prerelease_published_at,
            tester_roster,
            hotfix,
        )

    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    try:
        reservation = os.open(
            arguments.output,
            os.O_CREAT | os.O_EXCL | os.O_WRONLY,
            0o600,
        )
    except FileExistsError as error:
        raise ValueError(
            f"refusing to overwrite existing acceptance record: {arguments.output}"
        ) from error
    os.close(reservation)
    reserved_output = True
    temporary_path: Path | None = None
    try:
        descriptor, temporary_name = tempfile.mkstemp(
            prefix=f".{arguments.output.name}.",
            suffix=".tmp",
            dir=arguments.output.parent,
            text=True,
        )
        temporary_path = Path(temporary_name)
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as stream:
            json.dump(record, stream, indent=2, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary_path, arguments.output)
        reserved_output = False
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)
        if reserved_output:
            arguments.output.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--manifest-signature", type=Path, required=True)
    parser.add_argument("--signed-bundle", type=Path, required=True)
    parser.add_argument("--tester-roster", type=Path, required=True)
    parser.add_argument("--prerelease-published-at", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        create(arguments)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"acceptance scaffold failed: {error}", file=sys.stderr)
        return 1
    print(f"created not-run acceptance scaffold: {arguments.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
