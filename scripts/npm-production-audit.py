#!/usr/bin/env python3
"""Enforce that the published WASM/npm package has no production dependencies."""

import json
from pathlib import Path
import sys


root = Path(__file__).resolve().parents[1] / "prns-wasm"
package = json.loads((root / "package.json").read_text(encoding="utf-8"))
lock = json.loads((root / "package-lock.json").read_text(encoding="utf-8"))

if package.get("dependencies"):
    print("prns-wasm package.json must not declare production dependencies", file=sys.stderr)
    raise SystemExit(1)
if package.get("devDependencies") != {"typescript": "^5.9.3"}:
    print("TypeScript must remain the sole non-shipped npm development tool", file=sys.stderr)
    raise SystemExit(1)

root_lock = lock.get("packages", {}).get("", {})
if root_lock.get("dependencies"):
    print("prns-wasm lockfile resolved production dependencies", file=sys.stderr)
    raise SystemExit(1)
for path, metadata in lock.get("packages", {}).items():
    if path and not metadata.get("dev", False):
        print(f"non-development npm package entered production resolution: {path}", file=sys.stderr)
        raise SystemExit(1)

print("npm production resolution is empty; TypeScript is non-shipped tooling")
