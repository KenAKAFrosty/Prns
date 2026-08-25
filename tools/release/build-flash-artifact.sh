#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TARGET="${1:-}"
OUT_ROOT="${2:-$ROOT/target/flash-artifacts}"

usage() {
    echo "usage: $0 <board-slug|--all> [out-root]" >&2
    echo "board slugs come from the shipping entries in release/flash/boards.json" >&2
}

cd "$ROOT"
shipping_targets="$(
    cargo run --quiet --locked -p hopspot-flash -- list |
        awk '{print $1}'
)"
if [[ -z "$shipping_targets" ]]; then
    echo "shipping board catalog is empty" >&2
    exit 2
fi

build_target() {
    local board="$1"
    cargo run --locked -p hopspot-flash -- build "$board" --out-root "$OUT_ROOT"
}

case "$TARGET" in
    --all)
        while IFS= read -r board; do
            build_target "$board"
        done <<< "$shipping_targets"
        ;;
    "")
        usage
        exit 2
        ;;
    *)
        shipping=false
        while IFS= read -r board; do
            if [[ "$TARGET" == "$board" ]]; then
                shipping=true
                break
            fi
        done <<< "$shipping_targets"
        if [[ "$shipping" != true ]]; then
            echo "not a shipping board slug: $TARGET" >&2
            usage
            exit 2
        fi
        build_target "$TARGET"
        ;;
esac
