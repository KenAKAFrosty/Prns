#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-}"
OUT_ROOT="${2:-$ROOT/target/flash-artifacts}"

usage() {
    echo "usage: $0 <board-slug> [out-root]" >&2
    echo "supported board-slugs: heltec-v4, t-echo" >&2
}

case "$TARGET" in
    heltec-v4)
        cd "$ROOT"
        cargo run -p hopspot-flash -- build heltec-v4 --out-root "$OUT_ROOT"
        ;;
    t-echo)
        cd "$ROOT"
        cargo run -p hopspot-flash -- build t-echo --out-root "$OUT_ROOT"
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
