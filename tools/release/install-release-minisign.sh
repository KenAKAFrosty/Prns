#!/usr/bin/env bash
set -euo pipefail

version="0.12"
archive_sha256="9a599b48ba6eb7b1e80f12f36b94ceca7c00b7a5173c95c3efc88d9822957e73"
output_directory="${1:-}"
if [[ -z "$output_directory" ]]; then
    echo "usage: tools/release/install-release-minisign.sh OUTPUT_DIRECTORY" >&2
    exit 2
fi

case "$(uname -m)" in
    x86_64) binary_path="minisign-linux/x86_64/minisign" ;;
    aarch64|arm64) binary_path="minisign-linux/aarch64/minisign" ;;
    *) echo "unsupported Minisign release host architecture: $(uname -m)" >&2; exit 2 ;;
esac

temporary="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/prns-minisign.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
archive="$temporary/minisign-${version}-linux.tar.gz"
curl --fail --location --proto '=https' --tlsv1.2 \
    --output "$archive" \
    "https://github.com/jedisct1/minisign/releases/download/${version}/minisign-${version}-linux.tar.gz"
printf '%s  %s\n' "$archive_sha256" "$archive" | sha256sum --check --status
tar -xzf "$archive" -C "$temporary" "$binary_path"
mkdir -p "$output_directory"
install -m 0755 "$temporary/$binary_path" "$output_directory/minisign"
"$output_directory/minisign" -v
