#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
candidate="${1:-}"
acceptance="${2:-}"
signed_bundle="${3:-}"
qualification_evidence="${4:-}"
prerelease_published_at="${5:-}"
if [[ -z "$candidate" || ! -d "$candidate" ]]; then
    echo "usage: scripts/verify-flasher-candidate.sh CANDIDATE_DIR [ACCEPTANCE_JSON SIGNED_BUNDLE QUALIFICATION_EVIDENCE PRERELEASE_PUBLISHED_AT]" >&2
    exit 2
fi
optional=("$acceptance" "$signed_bundle" "$qualification_evidence" "$prerelease_published_at")
present=0
for value in "${optional[@]}"; do
    [[ -n "$value" ]] && present=$((present + 1))
done
if [[ "$present" -ne 0 && "$present" -ne "${#optional[@]}" ]]; then
    echo "acceptance validation requires its record, signed bundle, qualification evidence, and prerelease publishedAt" >&2
    exit 2
fi

python3 "$root/scripts/verify-flasher-candidate-files.py" "$candidate"
cargo run --quiet --locked -p prns-flash-manifest --bin validate-flasher-candidate -- "$candidate"
python3 "$root/scripts/flasher-website-history.py" validate-candidate \
    --candidate "$candidate"
version="$(tr -d '[:space:]' < "$candidate/VERSION")"
python3 "$root/scripts/validate-flasher-tester-roster.py" \
    --roster "$candidate/qualification/tester-roster.json" \
    --version "$version"
if [[ -n "$acceptance" ]]; then
    evidence_work="$(mktemp -d)"
    trap 'rm -rf "$evidence_work"' EXIT HUP INT TERM
    python3 "$root/scripts/extract-flasher-candidate.py" \
        "$qualification_evidence" "$evidence_work/root"
    python3 "$root/scripts/validate-flasher-acceptance.py" \
        --acceptance "$acceptance" \
        --manifest "$candidate/flash-manifest.json" \
        --manifest-signature "$candidate/flash-manifest.json.minisig" \
        --signed-bundle "$signed_bundle" \
        --tester-roster "$candidate/qualification/tester-roster.json" \
        --evidence-root "$evidence_work/root" \
        --prerelease-published-at "$prerelease_published_at"
fi

echo "FLASHER_SIGNED_CANDIDATE_VERIFIED"
