#!/usr/bin/env bash
# Reproduce the go-reticulum column: clone the pinned module, drop our harness into a
# subpackage, `go run` it, file the rows.
set -euo pipefail
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-256/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-256/go-reticulum.jsonl"

REPO="https://github.com/svanichkin/go-reticulum.git"
REF="06621cc"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

mkdir -p "$HERE/.upstream/announcebench"
cp "$HERE/main.go" "$HERE/.upstream/announcebench/main.go"
out="$(cd "$HERE/.upstream" && go run ./announcebench "$CORPUS")"
echo "$out"
read -r resolved per_sec <<<"$(parse_result "$out")"
toolchain="$(go version | awk '{print $3}')"
emit_rows "$OUT" "go-reticulum" "$REF" "$toolchain" "$HOST" "$resolved" "$per_sec"
