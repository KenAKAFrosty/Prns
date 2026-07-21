#!/usr/bin/env python3
"""Audit every production npm graph shipped by a PRNS release."""

import json
from pathlib import Path
import sys


repo = Path(__file__).resolve().parents[1]
root = repo / "prns-wasm"
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

print("prns-wasm npm production resolution is empty; TypeScript is non-shipped tooling")

site = repo / "docs" / "website"
site_package = json.loads((site / "package.json").read_text(encoding="utf-8"))
site_lock = json.loads((site / "package-lock.json").read_text(encoding="utf-8"))
expected_dependencies = {"esptool-js": "0.6.0", "spark-md5": "3.0.2"}
expected_dev = {
    "@tailwindcss/cli": "4.3.3",
    "esbuild": "0.28.1",
    "tailwindcss": "4.3.3",
}
if site_package.get("dependencies") != expected_dependencies:
    print("website production dependencies must remain exact-pinned esptool-js/spark-md5", file=sys.stderr)
    raise SystemExit(1)
if site_package.get("devDependencies") != expected_dev:
    print("website JavaScript build tools must remain exact-pinned", file=sys.stderr)
    raise SystemExit(1)

expected_production = {
    "node_modules/atob-lite": ("2.0.0", "MIT"),
    "node_modules/esptool-js": ("0.6.0", "Apache-2.0"),
    "node_modules/pako": ("2.2.0", "(MIT AND Zlib)"),
    "node_modules/spark-md5": ("3.0.2", "(WTFPL OR MIT)"),
    "node_modules/tslib": ("2.8.1", "0BSD"),
}
actual_production = {
    path: (metadata.get("version"), metadata.get("license"))
    for path, metadata in site_lock.get("packages", {}).items()
    if path and not metadata.get("dev", False)
}
if actual_production != expected_production:
    print(f"website production npm closure drifted: {actual_production!r}", file=sys.stderr)
    raise SystemExit(1)
root_lock = site_lock.get("packages", {}).get("", {})
if root_lock.get("dependencies") != expected_dependencies or root_lock.get("devDependencies") != expected_dev:
    print("website lockfile root disagrees with package.json exact pins", file=sys.stderr)
    raise SystemExit(1)

for source in (site / "src", site / "web-flasher"):
    for path in source.rglob("*"):
        if path.is_file() and path.suffix in {".rs", ".js", ".mjs", ".html", ".css"}:
            text = path.read_text(encoding="utf-8")
            if "unpkg.com" in text or "esp-web-tools" in text or "esp-web-install-button" in text:
                print(f"legacy web flasher/CDN reference remains in {path.relative_to(repo)}", file=sys.stderr)
                raise SystemExit(1)

print("website npm graph is exact-pinned, license-allowlisted, and free of runtime CDN/legacy-engine references")
