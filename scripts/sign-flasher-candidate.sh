#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
candidate="${1:-}"
secret_key="${2:-}"
if [[ -z "$candidate" || -z "$secret_key" ]]; then
    echo "usage: scripts/sign-flasher-candidate.sh CANDIDATE_DIR OFFLINE_MINISIGN_SECRET_KEY" >&2
    exit 2
fi
if [[ ! -d "$candidate" || ! -f "$secret_key" ]]; then
    echo "candidate directory or offline secret key is unavailable" >&2
    exit 2
fi
if ! command -v minisign >/dev/null 2>&1; then
    echo "minisign is required on the offline signing workstation" >&2
    exit 2
fi
if rg -q 'PRNS_RELEASE_KEY_NOT_CONFIGURED' "$root/release/keys/minisign.pub"; then
    echo "release/keys/minisign.pub still contains the fail-closed custody marker" >&2
    exit 4
fi
if ! cmp -s "$candidate/minisign.pub" "$root/release/keys/minisign.pub"; then
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
    minisign -S -s "$secret_key" -m "$document" -x "$document.minisig"
    minisign -Vm "$document" -x "$document.minisig" -p "$root/release/keys/minisign.pub"
done

release_dir="$candidate/website/releases/$version"
channel_dir="$candidate/website/releases/channels"
cp "$candidate/flash-manifest.json.minisig" "$release_dir/flash-manifest.json.minisig"
cp "$channel_file.minisig" "$channel_dir/$channel_name.json.minisig"

echo "Signed candidate $version with the offline Minisign key. The private key was not copied."
