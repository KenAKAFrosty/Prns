#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
validation_python="${VALIDATION_PYTHON:-python3}"

uvx --from ruff==0.15.22 ruff check \
    --select F821 \
    tools/release \
    tools/tests \
    validation/release
"$validation_python" validation/release/workflow-contracts.py
"$validation_python" validation/release/prnsd-feature-contracts.py
"$validation_python" validation/release/acceptance-doc-contracts.py
PYTHONDONTWRITEBYTECODE=1 "$validation_python" -m unittest discover \
    -s tools/tests \
    -p 'test_*.py' \
    -v
