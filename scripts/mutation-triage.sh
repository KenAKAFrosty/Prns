#!/usr/bin/env bash
# Run the configured cargo-mutants lane and print a compact survivor summary
# from the generated mutants.out directory. This keeps full mutation runs useful
# in CI logs while preserving the detailed per-mutant artifacts for review.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "${repo_root}"

output_root="${PRNS_MUTANTS_OUTPUT_ROOT:-.}"

set +e
cargo mutants --output "${output_root}" "$@"
status=$?
set -e

python3 - "${output_root}/mutants.out" <<'PY'
from pathlib import Path
import sys

root = Path(sys.argv[1])
print()
print("[mutation-triage] output:", root)
if not root.exists():
    print("[mutation-triage] no mutants.out directory was produced")
    sys.exit(0)

def read_lines(name):
    path = root / f"{name}.txt"
    if not path.exists():
        return []
    return [line.strip() for line in path.read_text(errors="replace").splitlines() if line.strip()]

for name in ("missed", "timeout", "unviable", "caught"):
    lines = read_lines(name)
    print(f"[mutation-triage] {name}: {len(lines)}")
    if name in {"missed", "timeout", "unviable"}:
        for line in lines[:20]:
            print(f"[mutation-triage]   {line}")
        if len(lines) > 20:
            print(f"[mutation-triage]   ... {len(lines) - 20} more")
PY

exit "${status}"
