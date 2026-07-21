#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
destination="${1:-}"
if [[ -z "$destination" ]]; then
    echo "usage: scripts/stage-release-notices.sh <distribution-directory>" >&2
    exit 2
fi
mkdir -p "$destination"
cp "$root/THIRD_PARTY_NOTICES.md" "$destination/THIRD_PARTY_NOTICES.md"
