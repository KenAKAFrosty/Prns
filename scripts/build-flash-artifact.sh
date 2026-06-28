#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-}"
OUT_ROOT="${2:-$ROOT/target/flash-artifacts}"

usage() {
    echo "usage: $0 <board-slug> [out-root]" >&2
    echo "supported board-slugs: t-echo" >&2
}

case "$TARGET" in
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
