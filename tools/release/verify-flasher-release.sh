#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
candidate=""
candidate_run=""
signed_bundle=""
acceptance=""
acceptance_source_commit=""
qualification_evidence=""
public_review_evidence=""
prerelease_published_at=""
release_record=""
attestation_bundle=""
attestation_metadata=""
repository=""
signer="${PRNS_MINISIGN_BIN:-minisign}"
public_key="${PRNS_MINISIGN_PUBLIC_KEY:-$root/release/keys/minisign.pub}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --candidate) candidate="${2:-}"; shift 2 ;;
        --candidate-run) candidate_run="${2:-}"; shift 2 ;;
        --signed-bundle) signed_bundle="${2:-}"; shift 2 ;;
        --acceptance) acceptance="${2:-}"; shift 2 ;;
        --acceptance-source-commit) acceptance_source_commit="${2:-}"; shift 2 ;;
        --qualification-evidence) qualification_evidence="${2:-}"; shift 2 ;;
        --public-review-evidence) public_review_evidence="${2:-}"; shift 2 ;;
        --prerelease-published-at) prerelease_published_at="${2:-}"; shift 2 ;;
        --release-record) release_record="${2:-}"; shift 2 ;;
        --attestation-bundle) attestation_bundle="${2:-}"; shift 2 ;;
        --attestation-metadata) attestation_metadata="${2:-}"; shift 2 ;;
        --repository) repository="${2:-}"; shift 2 ;;
        *) echo "unknown release verification argument: $1" >&2; exit 2 ;;
    esac
done

for required in \
    "$candidate" \
    "$candidate_run" \
    "$signed_bundle" \
    "$acceptance" \
    "$acceptance_source_commit" \
    "$qualification_evidence" \
    "$public_review_evidence" \
    "$prerelease_published_at" \
    "$release_record" \
    "$attestation_bundle" \
    "$attestation_metadata" \
    "$repository"; do
    if [[ -z "$required" ]]; then
        echo "complete signed release evidence arguments are required" >&2
        exit 2
    fi
done
if ! command -v "$signer" >/dev/null 2>&1; then
    echo "configured Minisign executable is unavailable: $signer" >&2
    exit 2
fi

"$root/tools/release/verify-flasher-candidate.sh" "$candidate"
"$signer" -Vm "$acceptance" -x "$acceptance.minisig" -p "$public_key"
"$signer" -Vm "$release_record" -x "$release_record.minisig" -p "$public_key"
evidence_work="$(mktemp -d)"
trap 'rm -rf "$evidence_work"' EXIT HUP INT TERM
python3 "$root/tools/release/extract-flasher-candidate.py" \
    "$qualification_evidence" "$evidence_work/root"
python3 "$candidate/qualification/validate-flasher-acceptance.py" \
    --acceptance "$acceptance" \
    --manifest "$candidate/flash-manifest.json" \
    --manifest-signature "$candidate/flash-manifest.json.minisig" \
    --signed-bundle "$signed_bundle" \
    --tester-roster "$candidate/qualification/tester-roster.json" \
    --evidence-root "$evidence_work/root" \
    --prerelease-published-at "$prerelease_published_at"
python3 "$root/tools/release/flasher-release-record.py" verify \
    --candidate "$candidate" \
    --candidate-run "$candidate_run" \
    --signed-bundle "$signed_bundle" \
    --acceptance "$acceptance" \
    --acceptance-source-commit "$acceptance_source_commit" \
    --qualification-evidence "$qualification_evidence" \
    --public-review-evidence "$public_review_evidence" \
    --prerelease-published-at "$prerelease_published_at" \
    --attestation-bundle "$attestation_bundle" \
    --attestation-metadata "$attestation_metadata" \
    --repository "$repository" \
    --release-record "$release_record"

echo "FLASHER_SIGNED_RELEASE_EVIDENCE_VERIFIED"
