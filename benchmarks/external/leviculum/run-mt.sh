#!/usr/bin/env bash
# Reproduce the Leviculum row of the announce-parallel comparison: clone the pinned
# upstream, build our parallel harness against its reticulum-core, sweep threads, file rows.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-parallel/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-parallel/leviculum.jsonl"

REPO="https://codeberg.org/Lew_Palm/leviculum.git"
REF="6f366ca"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

out="$(cargo run --quiet --release --manifest-path "$HERE/harness-mt/Cargo.toml" -- "$CORPUS")"
echo "$out"
read -r resolved lo lo_ps hi hi_ps <<<"$(parse_mt "$out")"
toolchain="$(rustc --version | sed 's/^rustc //')"
emit_mt_rows "$OUT" "Leviculum 0.6.3" "$REF" "$toolchain" "$HOST" "$resolved" "$lo" "$lo_ps" "$hi" "$hi_ps"
