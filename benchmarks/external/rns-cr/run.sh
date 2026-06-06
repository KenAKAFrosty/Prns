#!/usr/bin/env bash
# Reproduce the rns-cr column: clone the pinned shard, `shards install`, drop our bench
# into the repo root, `crystal run --release` it, file the rows.
set -euo pipefail
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-256/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-256/rns-cr.jsonl"

REPO="https://github.com/jtippett/rns-cr.git"
REF="514c309"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

cp "$HERE/bench.cr" "$HERE/.upstream/bench.cr"
( cd "$HERE/.upstream" && shards install --without-development --quiet )
out="$(cd "$HERE/.upstream" && crystal run --release --no-color bench.cr -- "$CORPUS")"
echo "$out"
read -r resolved per_sec <<<"$(parse_result "$out")"
toolchain="crystal $(crystal --version | sed -n 's/^Crystal \([0-9.]*\).*/\1/p')"
emit_rows "$OUT" "rns-cr 0.1.0" "$REF" "$toolchain" "$HOST" "$resolved" "$per_sec"
