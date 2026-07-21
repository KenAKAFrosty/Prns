#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
document="${1:-}"
secret_key="${2:-}"
signature="${3:-${document}.minisig}"
signer="${PRNS_MINISIGN_BIN:-minisign}"
public_key="${PRNS_MINISIGN_PUBLIC_KEY:-$root/release/keys/minisign.pub}"

if [[ -z "$document" || -z "$secret_key" ]]; then
    echo "usage: scripts/sign-flasher-document.sh DOCUMENT MINISIGN_SECRET_KEY [SIGNATURE]" >&2
    exit 2
fi
if [[ ! -f "$document" || ! -f "$secret_key" || ! -f "$public_key" ]]; then
    echo "document, secret key, or pinned public key is unavailable" >&2
    exit 2
fi
if [[ -e "$signature" ]]; then
    echo "refusing to replace existing signature: $signature" >&2
    exit 2
fi
if ! command -v "$signer" >/dev/null 2>&1; then
    echo "configured Minisign executable is unavailable: $signer" >&2
    exit 2
fi
if grep -q 'PRNS_RELEASE_KEY_NOT_CONFIGURED' "$public_key"; then
    echo "release public key still contains the fail-closed custody marker" >&2
    exit 4
fi

document_sha256="$(shasum -a 256 "$document" | awk '{print $1}')"
"$signer" -S -s "$secret_key" -m "$document" -x "$signature" \
    -t "prns-release-sha256:${document_sha256}"
"$signer" -Vm "$document" -x "$signature" -p "$public_key"
