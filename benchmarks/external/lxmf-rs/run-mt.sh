#!/usr/bin/env bash
# Reproduce the LXMF-rs row of the announce-parallel comparison: clone the pinned monorepo,
# drop our parallel example into its rns-core crate, sweep threads, file rows.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-parallel/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-parallel/lxmf-rs.jsonl"

REPO="https://github.com/FreeTAKTeam/LXMF-rs.git"
REF="30da190"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

mkdir -p "$HERE/.upstream/crates/libs/rns-core/examples"
cp "$HERE/announce_mt.rs" "$HERE/.upstream/crates/libs/rns-core/examples/announce_mt.rs"
out="$(cd "$HERE/.upstream" && cargo run --quiet --release --example announce_mt -p reticulum-rs-core -- "$CORPUS")"
echo "$out"
read -r resolved lo lo_ps hi hi_ps <<<"$(parse_mt "$out")"
toolchain="$(rustc --version | sed 's/^rustc //')"
emit_mt_rows "$OUT" "LXMF-rs 0.2.0" "$REF" "$toolchain" "$HOST" "$resolved" "$lo" "$lo_ps" "$hi" "$hi_ps"
