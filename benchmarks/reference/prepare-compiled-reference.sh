#!/usr/bin/env bash
set -euo pipefail

REFERENCE_DIR="$(cd "$(dirname "$0")" && pwd)"
VENV="$REFERENCE_DIR/.venv"
CACHE="$REFERENCE_DIR/.object-cache/uv"

mkdir -p "$CACHE"
UV_CACHE_DIR="$CACHE" uv venv --python 3.13 --allow-existing "$VENV"
UV_CACHE_DIR="$CACHE" uv pip sync --python "$VENV/bin/python" "$REFERENCE_DIR/requirements.lock"
"$VENV/bin/python" "$REFERENCE_DIR/compiled_reference.py" --verify-only
