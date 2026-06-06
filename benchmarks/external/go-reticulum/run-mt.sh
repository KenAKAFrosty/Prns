#!/usr/bin/env bash
# Reproduce the go-reticulum row of the announce-parallel comparison: clone the pinned
# module, drop our parallel harness into a subpackage, sweep threads, file rows.
set -euo pipefail
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-parallel/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-parallel/go-reticulum.jsonl"

REPO="https://github.com/svanichkin/go-reticulum.git"
REF="06621cc"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

mkdir -p "$HERE/.upstream/announcebenchmt"
cp "$HERE/main-mt.go" "$HERE/.upstream/announcebenchmt/main.go"
out="$(cd "$HERE/.upstream" && go run ./announcebenchmt "$CORPUS")"
echo "$out"
read -r resolved lo lo_ps hi hi_ps <<<"$(parse_mt "$out")"
toolchain="$(go version | awk '{print $3}')"
emit_mt_rows "$OUT" "go-reticulum" "$REF" "$toolchain" "$HOST" "$resolved" "$lo" "$lo_ps" "$hi" "$hi_ps"
