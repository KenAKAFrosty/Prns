from __future__ import annotations

from pathlib import Path
import subprocess
import sys

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "validation" / "manifest.toml"


def main() -> int:
    document = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    workspaces = document["registry"]["cargo_lock_workspaces"]
    failures = []
    for workspace in workspaces:
        manifest = ROOT / workspace / "Cargo.toml"
        result = subprocess.run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--no-deps",
                "--format-version",
                "1",
                "--manifest-path",
                str(manifest),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            detail = result.stderr.strip() or result.stdout.strip()
            failures.append((workspace, detail))
    if failures:
        print("stale or invalid first-party Cargo lockfiles:", file=sys.stderr)
        for workspace, detail in failures:
            print(f"\n{workspace}/Cargo.lock\n{detail}", file=sys.stderr)
        return 1
    print(f"validated {len(workspaces)} first-party Cargo lockfile workspaces")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
