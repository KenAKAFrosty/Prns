#!/usr/bin/env bash
# Reproduce the RetiNet column: clone the pinned fork, install it into an isolated venv
# (it ships the `RNS` module, so it must not collide with a system RNS), run, file rows.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-256/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-256/retinet.jsonl"

REPO="https://codeberg.org/skyguy/retinet.git"
REF="6039094"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

VENV="$HERE/.upstream/.venv"
if [ ! -x "$VENV/bin/python" ]; then
  python3 -m venv "$VENV"
  "$VENV/bin/pip" install -q --upgrade pip
  "$VENV/bin/pip" install -q "$HERE/.upstream"
fi

out="$("$VENV/bin/python" "$HERE/driver.py" "$CORPUS")"
echo "$out"
read -r resolved per_sec <<<"$(parse_result "$out")"
toolchain="$("$VENV/bin/python" -c 'import platform; print("CPython " + platform.python_version())')"
emit_rows "$OUT" "RetiNet 0.9.4" "$REF" "$toolchain" "$HOST" "$resolved" "$per_sec"
