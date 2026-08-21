#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TARGET="${1:-}"
OUT_ROOT="${2:-$ROOT/target/flash-artifacts}"

usage() {
    echo "usage: $0 <board-slug> [out-root]" >&2
    echo "supported board-slugs: heltec-v4, heltec-v4-r8, t-beam-supreme, xiao-esp32-c6, t-echo, t114" >&2
}

case "$TARGET" in
    heltec-v4)
        cd "$ROOT"
        cargo run --locked -p hopspot-flash -- build heltec-v4 --out-root "$OUT_ROOT"
        ;;
    heltec-v4-r8)
        cd "$ROOT"
        cargo run --locked -p hopspot-flash -- build heltec-v4-r8 --out-root "$OUT_ROOT"
        ;;
    t-beam-supreme)
        cd "$ROOT"
        cargo run --locked -p hopspot-flash -- build t-beam-supreme --out-root "$OUT_ROOT"
        ;;
    xiao-esp32-c6)
        cd "$ROOT"
        cargo run --locked -p hopspot-flash -- build xiao-esp32-c6 --out-root "$OUT_ROOT"
        ;;
    t-echo)
        cd "$ROOT"
        cargo run --locked -p hopspot-flash -- build t-echo --out-root "$OUT_ROOT"
        ;;
    t114)
        cd "$ROOT"
        cargo run --locked -p hopspot-flash -- build t114 --out-root "$OUT_ROOT"
        ;;
    "")
        usage
        exit 2
        ;;
    *)
        usage
        exit 2
        ;;
esac
