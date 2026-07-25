#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$root/tools/release/release-esp-toolchain-identity.sh"

if [[ "$(espup --version)" != "espup $ESPUP_VERSION" ]]; then
    echo "installed espup identity does not match $ESPUP_VERSION" >&2
    exit 4
fi

verbose="$(rustc +esp -vV)"
banner="${verbose%%$'\n'*}"
release="$(sed -n 's/^release: //p' <<<"$verbose")"
commit_hash="$(sed -n 's/^commit-hash: //p' <<<"$verbose")"
commit_date="$(sed -n 's/^commit-date: //p' <<<"$verbose")"
if [[ "$banner" != "$ESP_RUSTC_BANNER" ]] \
    || [[ "$release" != "$ESP_RUSTC_RELEASE" ]] \
    || [[ "$commit_hash" != "$ESP_RUSTC_COMMIT_HASH" ]] \
    || [[ "$commit_date" != "$ESP_RUSTC_COMMIT_DATE" ]]; then
    echo "installed ESP Rust compiler identity does not match $ESP_RUST_TOOLCHAIN_VERSION" >&2
    exit 4
fi

if [[ "$(xtensa-esp-elf-gcc --version | head -n 1)" != "$ESP_GCC_BANNER" ]]; then
    echo "installed Xtensa GCC identity does not match $ESP_CROSSTOOL_VERSION" >&2
    exit 4
fi

printf 'verified exact release ESP tools: espup %s, %s, crosstool-NG %s\n' \
    "$ESPUP_VERSION" "$ESP_RUSTC_BANNER" "$ESP_CROSSTOOL_VERSION"
