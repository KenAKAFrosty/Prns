#!/usr/bin/env python3
"""List every firmware artifact in a signed flash manifest."""

from __future__ import annotations

import argparse
import json
from pathlib import Path, PurePosixPath
import sys

from flasher_manifest import require_schema, target_artifacts


def artifacts(manifest: dict) -> list[dict[str, str | int]]:
    require_schema(manifest)
    targets = manifest.get("targets")
    if not isinstance(targets, list) or not targets:
        raise ValueError("flash manifest has no targets")
    output = []
    seen_paths = set()
    for target in targets:
        if not isinstance(target, dict):
            raise ValueError("flash manifest contains a malformed target")
        board_slug = target.get("board_slug")
        if not isinstance(board_slug, str) or not board_slug:
            raise ValueError("flash manifest target has no board slug")
        for artifact in target_artifacts(target):
            path = artifact.get("path")
            size = artifact.get("size")
            checksum = artifact.get("sha256")
            pure = PurePosixPath(path) if isinstance(path, str) else None
            if (
                pure is None
                or "\\" in path
                or pure.is_absolute()
                or not pure.parts
                or any(part in {"", ".", ".."} for part in pure.parts)
                or pure.as_posix() != path
                or path in seen_paths
                or not isinstance(size, int)
                or isinstance(size, bool)
                or size <= 0
                or not isinstance(checksum, str)
                or len(checksum) != 64
                or any(character not in "0123456789abcdef" for character in checksum)
            ):
                raise ValueError("flash manifest contains an invalid artifact identity")
            seen_paths.add(path)
            output.append(
                {
                    "board_slug": board_slug,
                    "path": path,
                    "size": size,
                    "sha256": checksum,
                }
            )
    return sorted(output, key=lambda item: str(item["path"]))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--format", choices=("paths", "identities"), default="paths")
    arguments = parser.parse_args()
    try:
        manifest = json.loads(arguments.manifest.read_text(encoding="utf-8"))
        if not isinstance(manifest, dict):
            raise ValueError("flash manifest must be a JSON object")
        for artifact in artifacts(manifest):
            if arguments.format == "identities":
                print(
                    f"{artifact['path']}\t{artifact['size']}\t{artifact['sha256']}"
                )
            else:
                print(artifact["path"])
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"flash manifest artifact listing failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
