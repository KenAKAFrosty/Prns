#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
candidate="${1:-}"
acceptance="${2:-}"
if [[ -z "$candidate" || ! -d "$candidate" ]]; then
    echo "usage: scripts/verify-flasher-candidate.sh CANDIDATE_DIR [ACCEPTANCE_JSON]" >&2
    exit 2
fi

cargo run --quiet --locked -p prns-flash-manifest --bin validate-flasher-candidate -- "$candidate"
if [[ -n "$acceptance" ]]; then
    python3 "$root/scripts/validate-flasher-acceptance.py" \
        --acceptance "$acceptance" \
        --manifest "$candidate/flash-manifest.json" \
        --manifest-signature "$candidate/flash-manifest.json.minisig"
fi

echo "FLASHER_SIGNED_CANDIDATE_VERIFIED"
