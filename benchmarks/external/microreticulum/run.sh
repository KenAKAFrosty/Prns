#!/usr/bin/env bash
# Reproduce the microReticulum column: clone the pinned upstream, CMake-build our harness
# against it (FetchContent pulls its Crypto/microStore deps), run, file the rows.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-256/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-256/microreticulum.jsonl"

REPO="https://github.com/attermann/microReticulum.git"
REF="79b8524"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

cmake -S "$HERE" -B "$HERE/build" -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$HERE/build" -j8 >/dev/null
out="$("$HERE/build/mr_announce_bench" "$CORPUS")"
echo "$out"
read -r resolved per_sec <<<"$(parse_result "$out")"
toolchain="$(clang++ --version | head -1)"
emit_rows "$OUT" "microReticulum" "$REF" "$toolchain" "$HOST" "$resolved" "$per_sec"
