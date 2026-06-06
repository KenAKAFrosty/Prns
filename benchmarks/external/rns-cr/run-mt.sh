#!/usr/bin/env bash
# Reproduce the rns-cr row of the announce-parallel comparison: clone the pinned shard,
# `shards install`, drop our parallel bench into the repo root, build it with -Dpreview_mt
# and CRYSTAL_WORKERS pinned to the core count so fibers spread across OS threads, file rows.
set -euo pipefail
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-parallel/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-parallel/rns-cr.jsonl"
export CRYSTAL_WORKERS="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"

REPO="https://github.com/jtippett/rns-cr.git"
REF="514c309"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

cp "$HERE/bench_mt.cr" "$HERE/.upstream/bench_mt.cr"
( cd "$HERE/.upstream" && shards install --without-development --quiet )
out="$(cd "$HERE/.upstream" && crystal run --release -Dpreview_mt --no-color bench_mt.cr -- "$CORPUS")"
echo "$out"
read -r resolved lo lo_ps hi hi_ps <<<"$(parse_mt "$out")"
toolchain="crystal $(crystal --version | sed -n 's/^Crystal \([0-9.]*\).*/\1/p') (preview_mt)"
emit_mt_rows "$OUT" "rns-cr 0.1.0" "$REF" "$toolchain" "$HOST" "$resolved" "$lo" "$lo_ps" "$hi" "$hi_ps" "announces_verified"
