#!/usr/bin/env bash
# Reproduce the LXMF-rs column: clone the pinned monorepo, drop our example into its
# reticulum-rs-core crate (so the heavy workspace inheritance resolves), run, file rows.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-256/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-256/lxmf-rs.jsonl"

REPO="https://github.com/FreeTAKTeam/LXMF-rs.git"
REF="30da190"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

mkdir -p "$HERE/.upstream/crates/libs/rns-core/examples"
cp "$HERE/announce_bench.rs" "$HERE/.upstream/crates/libs/rns-core/examples/announce_bench.rs"
out="$(cd "$HERE/.upstream" && cargo run --quiet --release --example announce_bench -p reticulum-rs-core -- "$CORPUS")"
echo "$out"
read -r resolved per_sec <<<"$(parse_result "$out")"
toolchain="$(rustc --version | sed 's/^rustc //')"
emit_rows "$OUT" "LXMF-rs 0.2.0" "$REF" "$toolchain" "$HOST" "$resolved" "$per_sec"
