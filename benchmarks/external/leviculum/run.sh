#!/usr/bin/env bash
# Reproduce the Leviculum column of the announce-256 comparison: clone the pinned
# upstream, build our harness against its reticulum-core, run it, file the rows.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-256/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-256/leviculum.jsonl"

REPO="https://codeberg.org/Lew_Palm/leviculum.git"
REF="6f366ca"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

out="$(cargo run --quiet --release --manifest-path "$HERE/harness/Cargo.toml" -- "$CORPUS")"
echo "$out"
read -r resolved per_sec <<<"$(parse_result "$out")"
toolchain="$(rustc --version | sed 's/^rustc //')"
emit_rows "$OUT" "Leviculum 0.6.3" "$REF" "$toolchain" "$HOST" "$resolved" "$per_sec"
