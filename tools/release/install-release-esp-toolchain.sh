#!/usr/bin/env bash
set -euo pipefail

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

espup_version="0.17.1"
esp_rust_version="1.95.0.0"
espup_sha256="dbe54e9907b687809dbe1b955731569ed6df2b525362710d676256c5c8cf9ccd"
espup_url="https://github.com/esp-rs/espup/releases/download/v${espup_version}/espup-x86_64-unknown-linux-gnu"
crosstool_version="15.2.0_20250920"
gcc_archive="xtensa-esp-elf-${crosstool_version}-x86_64-linux-gnu.tar.xz"
gcc_sha256="e3d77ad14544814527bbe7a2d0f79ec4592a4e23392c51c7388c0e686b6a6977"
gcc_url="https://github.com/espressif/crosstool-NG/releases/download/esp-${crosstool_version}/${gcc_archive}"
gcc_banner="xtensa-esp-elf-gcc (crosstool-NG esp-${crosstool_version}) 15.2.0"
temporary="$(mktemp -d "${RUNNER_TEMP:-/tmp}/prns-espup.XXXXXX")"
trap 'rm -rf -- "$temporary"' EXIT HUP INT TERM
mkdir -p "$destination"

curl --fail --location --proto '=https' --tlsv1.2 --output "$temporary/espup" "$espup_url"
actual="$(sha256sum "$temporary/espup" | awk '{print $1}')"
if [[ "$actual" != "$espup_sha256" ]]; then
    echo "espup ${espup_version} SHA-256 mismatch" >&2
    exit 4
fi
install -m 0755 "$temporary/espup" "$destination/espup"
if [[ "$("$destination/espup" --version)" != "espup ${espup_version}" ]]; then
    echo "installed espup version does not match ${espup_version}" >&2
    exit 4
fi

export ESPUP_EXPORT_FILE="$destination/export-esp.sh"
"$destination/espup" install \
    --std \
    --targets esp32s3 \
    --toolchain-version "$esp_rust_version" \
    --crosstool-toolchain-version "$crosstool_version"
test -s "$ESPUP_EXPORT_FILE"

curl --fail --location --proto '=https' --tlsv1.2 \
    --output "$temporary/$gcc_archive" "$gcc_url"
actual="$(sha256sum "$temporary/$gcc_archive" | awk '{print $1}')"
if [[ "$actual" != "$gcc_sha256" ]]; then
    echo "Espressif crosstool-NG ${crosstool_version} SHA-256 mismatch" >&2
    exit 4
fi
rustup_home="$(rustup show home)"
gcc_destination="$rustup_home/toolchains/esp/xtensa-esp-elf/esp-${crosstool_version}"
if [[ -e "$gcc_destination" ]]; then
    echo "refusing to reuse an unverified Xtensa GCC destination: $gcc_destination" >&2
    exit 4
fi
mkdir -p "$gcc_destination"
tar -xJf "$temporary/$gcc_archive" -C "$gcc_destination"
gcc_bin="$gcc_destination/xtensa-esp-elf/bin/xtensa-esp-elf-gcc"
test -x "$gcc_bin"
if [[ "$("$gcc_bin" --version | head -n 1)" != "$gcc_banner" ]]; then
    echo "installed Xtensa GCC identity does not match ${crosstool_version}" >&2
    exit 4
fi
printf 'export PATH="%s:$PATH"\n' "$(dirname "$gcc_bin")" >> "$ESPUP_EXPORT_FILE"
# shellcheck disable=SC1090
source "$ESPUP_EXPORT_FILE"
export PATH="$destination:$PATH"

if ! rustc +esp -vV | rg -q '^release: 1\.95\.0'; then
    echo "installed ESP Rust compiler does not match ${esp_rust_version}" >&2
    exit 4
fi
if [[ "$(xtensa-esp-elf-gcc --version | head -n 1)" != "$gcc_banner" ]]; then
    echo "exact ESP toolchain did not provide the pinned Xtensa GCC" >&2
    exit 4
fi

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
    "$espup_version" "$esp_rust_version" "$crosstool_version"
