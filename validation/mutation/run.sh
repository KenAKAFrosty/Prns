#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${repo_root}"

output_root="${PRNS_MUTANTS_OUTPUT_ROOT:-.}"

set +e
cargo mutants \
    --config validation/mutation/config.toml \
    --output "${output_root}" \
    "$@"
status=$?
set -e

case "$status" in
    0|2|3)
        ;;
    *)
        echo "[mutation-triage] cargo-mutants failed with non-reviewable status ${status}" >&2
        exit "$status"
        ;;
esac

python3 - "${output_root}/mutants.out" <<'PY'
from pathlib import Path
import sys

root = Path(sys.argv[1])
print()
print("[mutation-triage] output:", root)
if not root.exists():
    print("[mutation-triage] no mutants.out directory was produced")
    sys.exit(1)

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

results="${output_root}/mutants.out/outcomes.json"
if [[ ! -f "$results" ]]; then
    echo "[mutation-triage] cargo-mutants exited ${status} without outcomes.json" >&2
    exit 1
fi

python3 validation/run.py mutation-shard-check --results "$results"
