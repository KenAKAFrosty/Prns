#!/usr/bin/env bash
# Reproduce the microReticulum row of the announce-parallel comparison: clone the pinned
# upstream, CMake-build our parallel harness against it, sweep threads, file rows.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../lib.sh"
BENCH_DIR="$(cd "$HERE/../.." && pwd)"
CORPUS="$BENCH_DIR/scenarios/announce-parallel/packets.hex"
HOST="$(rustc_host)"
OUT="$BENCH_DIR/results/$HOST/announce-parallel/microreticulum.jsonl"

REPO="https://github.com/attermann/microReticulum.git"
REF="79b8524"
clone_pinned "$REPO" "$REF" "$HERE/.upstream"

cmake -S "$HERE/mt" -B "$HERE/build-mt" -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$HERE/build-mt" -j8 >/dev/null
out="$("$HERE/build-mt/mr_announce_mt" "$CORPUS")"
echo "$out"
read -r resolved lo lo_ps hi hi_ps <<<"$(parse_mt "$out")"
toolchain="$(clang++ --version | head -1)"
emit_mt_rows "$OUT" "microReticulum" "$REF" "$toolchain" "$HOST" "$resolved" "$lo" "$lo_ps" "$hi" "$hi_ps" "announces_verified"
