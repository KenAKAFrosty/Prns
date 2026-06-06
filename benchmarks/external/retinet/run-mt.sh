#!/usr/bin/env bash
# Reproduce the RetiNet row of the announce-parallel comparison: clone the pinned fork into
# its isolated venv, sweep threads, file rows.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-parallel/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-parallel/retinet.jsonl"

REPO="https://codeberg.org/skyguy/retinet.git"
REF="6039094"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

VENV="$HERE/.upstream/.venv"
if [ ! -x "$VENV/bin/python" ]; then
  python3 -m venv "$VENV"
  "$VENV/bin/pip" install -q --upgrade pip
  "$VENV/bin/pip" install -q "$HERE/.upstream"
fi

out="$("$VENV/bin/python" "$HERE/driver-mt.py" "$CORPUS")"
echo "$out"
read -r resolved lo lo_ps hi hi_ps <<<"$(parse_mt "$out")"
toolchain="$("$VENV/bin/python" -c 'import platform; print("CPython " + platform.python_version())')"
emit_mt_rows "$OUT" "RetiNet 0.9.4" "$REF" "$toolchain" "$HOST" "$resolved" "$lo" "$lo_ps" "$hi" "$hi_ps"
