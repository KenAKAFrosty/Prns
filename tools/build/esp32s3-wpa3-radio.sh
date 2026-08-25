#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
vendor="$root/personal-hopspot/embedded/esp32/vendor/esp-wifi-sys-esp32s3-0.2.0"
recipe="$vendor/prns-build"
source "$recipe/identity.env"

usage() {
    echo "usage: ./tools/prns build esp32s3-wpa3-radio [--write]" >&2
    exit 2
}

write=false
case "${1:-}" in
    "") ;;
    --write) write=true ;;
    *) usage ;;
esac
[[ $# -le 1 ]] || usage

if [[ -z "${IDF_PATH:-}" ]] || [[ ! -f "$IDF_PATH/tools/idf.py" ]]; then
    echo "IDF_PATH must name an ESP-IDF $ESP_IDF_VERSION checkout" >&2
    exit 4
fi

idf_revision="$(git -C "$IDF_PATH" rev-parse HEAD)"
if [[ "$idf_revision" != "$ESP_IDF_REVISION" ]]; then
    echo "ESP-IDF revision $idf_revision does not match $ESP_IDF_REVISION" >&2
    exit 4
fi
if [[ -n "$(git -C "$IDF_PATH" status --porcelain --untracked-files=no)" ]]; then
    echo "ESP-IDF checkout has tracked modifications" >&2
    exit 4
fi

mbedtls="$IDF_PATH/components/mbedtls/mbedtls"
mbedtls_revision="$(git -C "$mbedtls" rev-parse HEAD)"
if [[ "$mbedtls_revision" != "$MBEDTLS_REVISION" ]]; then
    echo "Mbed TLS revision $mbedtls_revision does not match $MBEDTLS_REVISION" >&2
    exit 4
fi
if [[ -n "$(git -C "$mbedtls" status --porcelain --untracked-files=no)" ]]; then
    echo "Mbed TLS checkout has tracked modifications" >&2
    exit 4
fi

gcc="$(command -v xtensa-esp-elf-gcc || true)"
if [[ -z "$gcc" ]]; then
    echo "xtensa-esp-elf-gcc $ESP_CROSSTOOL_VERSION is required" >&2
    exit 4
fi
gcc_banner="$($gcc --version | sed -n '1p')"
if [[ "$gcc_banner" != "$ESP_GCC_BANNER" ]]; then
    echo "Xtensa GCC identity does not match $ESP_CROSSTOOL_VERSION" >&2
    echo "found: $gcc_banner" >&2
    exit 4
fi

build="$root/.build/esp32s3-wpa3-radio"
rm -rf "$build"
mkdir -p "$build"

IDF_COMPONENT_MANAGER=0 python3 "$IDF_PATH/tools/idf.py" \
    -C "$recipe/project" \
    -B "$build" \
    -G "Unix Makefiles" \
    -D "SDKCONFIG=$build/sdkconfig" \
    -D "SDKCONFIG_DEFAULTS=$recipe/sdkconfig.defaults" \
    set-target esp32s3 build

built_wpa="$build/esp-idf/wpa_supplicant/libwpa_supplicant.a"
built_mbed="$build/esp-idf/mbedtls/mbedtls/library/libmbedcrypto.a"
for archive in "$built_wpa" "$built_mbed"; do
    if [[ ! -f "$archive" ]]; then
        echo "expected archive was not produced: $archive" >&2
        exit 5
    fi
done

require_config() {
    if ! grep -qxF "$1" "$build/sdkconfig"; then
        echo "generated sdkconfig does not contain: $1" >&2
        exit 5
    fi
}
for setting in \
    "CONFIG_ESP_WIFI_ENABLE_WPA3_SAE=y" \
    "CONFIG_ESP_WIFI_ENABLE_SAE_H2E=y" \
    "CONFIG_ESP_WIFI_SOFTAP_SAE_SUPPORT=y" \
    "CONFIG_ESP_WIFI_MBEDTLS_CRYPTO=y" \
    "CONFIG_MBEDTLS_INTERNAL_MEM_ALLOC=y" \
    "# CONFIG_ESP_WIFI_ENABLE_SAE_PK is not set" \
    "# CONFIG_ESP_WIFI_ENABLE_WPA3_OWE_STA is not set" \
    "# CONFIG_ESP_WIFI_DPP_SUPPORT is not set" \
    "# CONFIG_ESP_WIFI_MBEDTLS_TLS_CLIENT is not set" \
    "# CONFIG_MBEDTLS_HARDWARE_AES is not set" \
    "# CONFIG_MBEDTLS_HARDWARE_MPI is not set" \
    "# CONFIG_MBEDTLS_HARDWARE_SHA is not set"; do
    require_config "$setting"
done

sha256() {
    python3 -c 'import hashlib, pathlib, sys; print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())' "$1"
}

wpa_hash="$(sha256 "$built_wpa")"
mbed_hash="$(sha256 "$built_mbed")"
if [[ "$wpa_hash" != "$WPA_SUPPLICANT_SHA256" ]]; then
    echo "libwpa_supplicant.a hash $wpa_hash does not match $WPA_SUPPLICANT_SHA256" >&2
    exit 5
fi
if [[ "$mbed_hash" != "$MBEDCRYPTO_SHA256" ]]; then
    echo "libmbedcrypto.a hash $mbed_hash does not match $MBEDCRYPTO_SHA256" >&2
    exit 5
fi

if $write; then
    cp "$built_wpa" "$vendor/libs/libwpa_supplicant.a"
    cp "$built_mbed" "$vendor/libs/libmbedcrypto.a"
else
    cmp -s "$built_wpa" "$vendor/libs/libwpa_supplicant.a" || {
        echo "vendored libwpa_supplicant.a differs from the verified rebuild" >&2
        exit 5
    }
    cmp -s "$built_mbed" "$vendor/libs/libmbedcrypto.a" || {
        echo "vendored libmbedcrypto.a differs from the verified rebuild" >&2
        exit 5
    }
fi

printf 'verified ESP32-S3 WPA3 radio archives: ESP-IDF %s, Mbed TLS %.7s, %s\n' \
    "$ESP_IDF_VERSION@$ESP_IDF_REVISION" "$MBEDTLS_REVISION" "$ESP_CROSSTOOL_VERSION"
