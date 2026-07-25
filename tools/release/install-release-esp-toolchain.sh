#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$root/tools/release/release-esp-toolchain-identity.sh"

destination="${1:-}"
if [[ -z "$destination" ]]; then
    echo "usage: tools/release/install-release-esp-toolchain.sh DESTINATION" >&2
    exit 2
fi
if [[ "$(uname -s)-$(uname -m)" != "Linux-x86_64" ]]; then
    echo "the release ESP installer is pinned for the ubuntu-24.04 x86_64 candidate runner" >&2
    exit 2
fi
if [[ -e "$destination" ]] && [[ -n "$(find "$destination" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)" ]]; then
    echo "release ESP tool destination must be new or empty: $destination" >&2
    exit 2
fi

espup_sha256="dbe54e9907b687809dbe1b955731569ed6df2b525362710d676256c5c8cf9ccd"
espup_url="https://github.com/esp-rs/espup/releases/download/v${ESPUP_VERSION}/espup-x86_64-unknown-linux-gnu"
gcc_archive="xtensa-esp-elf-${ESP_CROSSTOOL_VERSION}-x86_64-linux-gnu.tar.xz"
gcc_sha256="e3d77ad14544814527bbe7a2d0f79ec4592a4e23392c51c7388c0e686b6a6977"
gcc_url="https://github.com/espressif/crosstool-NG/releases/download/esp-${ESP_CROSSTOOL_VERSION}/${gcc_archive}"
temporary="$(mktemp -d "${RUNNER_TEMP:-/tmp}/prns-espup.XXXXXX")"
trap 'rm -rf -- "$temporary"' EXIT HUP INT TERM
mkdir -p "$destination"

curl --fail --location --proto '=https' --tlsv1.2 --output "$temporary/espup" "$espup_url"
actual="$(sha256sum "$temporary/espup" | awk '{print $1}')"
if [[ "$actual" != "$espup_sha256" ]]; then
    echo "espup ${ESPUP_VERSION} SHA-256 mismatch" >&2
    exit 4
fi
install -m 0755 "$temporary/espup" "$destination/espup"
if [[ "$("$destination/espup" --version)" != "espup ${ESPUP_VERSION}" ]]; then
    echo "installed espup version does not match ${ESPUP_VERSION}" >&2
    exit 4
fi

export ESPUP_EXPORT_FILE="$destination/export-esp.sh"
"$destination/espup" install \
    --std \
    --targets esp32s3 \
    --toolchain-version "$ESP_RUST_TOOLCHAIN_VERSION" \
    --crosstool-toolchain-version "$ESP_CROSSTOOL_VERSION"
test -s "$ESPUP_EXPORT_FILE"

curl --fail --location --proto '=https' --tlsv1.2 \
    --output "$temporary/$gcc_archive" "$gcc_url"
actual="$(sha256sum "$temporary/$gcc_archive" | awk '{print $1}')"
if [[ "$actual" != "$gcc_sha256" ]]; then
    echo "Espressif crosstool-NG ${ESP_CROSSTOOL_VERSION} SHA-256 mismatch" >&2
    exit 4
fi
rustup_home="$(rustup show home)"
gcc_destination="$rustup_home/toolchains/esp/xtensa-esp-elf/esp-${ESP_CROSSTOOL_VERSION}"
if [[ -e "$gcc_destination" ]]; then
    echo "refusing to reuse an unverified Xtensa GCC destination: $gcc_destination" >&2
    exit 4
fi
mkdir -p "$gcc_destination"
tar -xJf "$temporary/$gcc_archive" -C "$gcc_destination"
gcc_bin="$gcc_destination/xtensa-esp-elf/bin/xtensa-esp-elf-gcc"
test -x "$gcc_bin"
if [[ "$("$gcc_bin" --version | head -n 1)" != "$ESP_GCC_BANNER" ]]; then
    echo "installed Xtensa GCC identity does not match ${ESP_CROSSTOOL_VERSION}" >&2
    exit 4
fi
printf 'export PATH="%s:$PATH"\n' "$(dirname "$gcc_bin")" >> "$ESPUP_EXPORT_FILE"
# shellcheck disable=SC1090
source "$ESPUP_EXPORT_FILE"
export PATH="$destination:$PATH"

"$root/tools/release/verify-release-esp-toolchain.sh"

if [[ -n "${GITHUB_PATH:-}" ]]; then
    printf '%s\n' "$destination" >> "$GITHUB_PATH"
    while IFS= read -r path; do
        test -n "$path" && printf '%s\n' "$path" >> "$GITHUB_PATH"
    done < <(printf '%s' "$PATH" | tr ':' '\n')
fi
if [[ -n "${GITHUB_ENV:-}" ]]; then
    printf 'ESPUP_EXPORT_FILE=%s\n' "$ESPUP_EXPORT_FILE" >> "$GITHUB_ENV"
    if [[ -n "${LIBCLANG_PATH:-}" ]]; then
        printf 'LIBCLANG_PATH=%s\n' "$LIBCLANG_PATH" >> "$GITHUB_ENV"
    fi
fi

printf 'installed exact release ESP tools: espup %s, ESP Rust %s, crosstool-NG %s\n' \
    "$ESPUP_VERSION" "$ESP_RUST_TOOLCHAIN_VERSION" "$ESP_CROSSTOOL_VERSION"
