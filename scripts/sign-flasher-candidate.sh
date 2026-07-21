#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
candidate="${1:-}"
secret_key="${2:-}"
public_key="${PRNS_MINISIGN_PUBLIC_KEY:-$root/release/keys/minisign.pub}"
if [[ -z "$candidate" || -z "$secret_key" ]]; then
    echo "usage: scripts/sign-flasher-candidate.sh CANDIDATE_DIR MINISIGN_SECRET_KEY" >&2
    exit 2
fi
if [[ ! -d "$candidate" || ! -f "$secret_key" ]]; then
    echo "candidate directory or offline secret key is unavailable" >&2
    exit 2
fi
if grep -q 'PRNS_RELEASE_KEY_NOT_CONFIGURED' "$public_key"; then
    echo "release public key still contains the fail-closed custody marker" >&2
    exit 4
fi
if ! cmp -s "$candidate/minisign.pub" "$public_key"; then
    echo "candidate public key differs from the repository-pinned release key" >&2
    exit 4
fi

channel_files=("$candidate"/channels/*.json)
if [[ ! -e "${channel_files[0]}" ]] || [[ "${#channel_files[@]}" -ne 1 ]]; then
    echo "candidate must contain exactly one channel descriptor" >&2
    exit 2
fi
channel_file="${channel_files[0]}"
version="$(tr -d '[:space:]' < "$candidate/VERSION")"
channel_name="$(basename "$channel_file" .json)"

documents=(
    "$candidate/flash-manifest.json"
    "$channel_file"
    "$candidate/SHA256SUMS.txt"
)
for document in "${documents[@]}"; do
    if [[ ! -f "$document" || -e "$document.minisig" ]]; then
        echo "missing document or existing signature: $document" >&2
        exit 2
    fi
    "$root/scripts/sign-flasher-document.sh" "$document" "$secret_key"
done

release_dir="$candidate/website/releases/$version"
channel_dir="$candidate/website/releases/channels"
cp "$candidate/flash-manifest.json.minisig" "$release_dir/flash-manifest.json.minisig"
cp "$channel_file.minisig" "$channel_dir/$channel_name.json.minisig"

echo "Signed exact candidate $version. The private key was not copied into the candidate."
