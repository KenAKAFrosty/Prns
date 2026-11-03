#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ "$(cargo deny --version 2>/dev/null || true)" != "cargo-deny 0.19.8" ]]; then
    echo "cargo-deny 0.19.8 is required: cargo install cargo-deny --version 0.19.8 --locked" >&2
    exit 2
fi
if [[ "$(cargo about --version 2>/dev/null || true)" != "cargo-about 0.9.1" ]]; then
    echo "cargo-about 0.9.1 is required: cargo install cargo-about --version 0.9.1 --locked --features cli" >&2
    exit 2
fi

python3 "$root/validation/security/license-policy-parity.py"

graphs=(
    "group-headless-peer|prns-groups/Cargo.toml|x86_64-unknown-linux-gnu|headless"

    "group-mls-adapter|prns-groups/security/mls-rs/Cargo.toml|x86_64-unknown-linux-gnu"
    "group-libraries|prns-groups/Cargo.toml|x86_64-unknown-linux-gnu"
    "group-secure-storage|prns-groups/Cargo.toml|x86_64-unknown-linux-gnu|prns-groups-store/secure"
    "identity-keys-linux|prns-identities/bindings/uniffi/Cargo.toml|x86_64-unknown-linux-gnu"
    "identity-keys-macos|prns-identities/bindings/uniffi/Cargo.toml|aarch64-apple-darwin"
    "identity-catalog-linux|prns-identities/bindings/uniffi/Cargo.toml|x86_64-unknown-linux-gnu|catalog"
    "identity-catalog-macos|prns-identities/bindings/uniffi/Cargo.toml|aarch64-apple-darwin|catalog"
    "identity-key-card-linux|prns-identities/bindings/uniffi/Cargo.toml|x86_64-unknown-linux-gnu|contact-card"
    "identity-key-card-macos|prns-identities/bindings/uniffi/Cargo.toml|aarch64-apple-darwin|contact-card"
    "identity-catalog-card-linux|prns-identities/bindings/uniffi/Cargo.toml|x86_64-unknown-linux-gnu|catalog contact-card"
    "identity-catalog-card-macos|prns-identities/bindings/uniffi/Cargo.toml|aarch64-apple-darwin|catalog contact-card"
    "contact-card|prns-contact-card/Cargo.toml|x86_64-unknown-linux-gnu"
    "identity-storage|prns-identities/Cargo.toml|x86_64-unknown-linux-gnu|redb-store"
    "storage-redb|prns-storage/Cargo.toml|x86_64-unknown-linux-gnu|redb"
    "engine|Cargo.toml|x86_64-unknown-linux-gnu"
    "daemon-linux|prnsd/Cargo.toml|x86_64-unknown-linux-gnu"
    "daemon-macos|prnsd/Cargo.toml|aarch64-apple-darwin"
    "daemon-windows|prnsd/Cargo.toml|x86_64-pc-windows-msvc"
    "desktop-linux|personal-hopspot/desktop/Cargo.toml|x86_64-unknown-linux-gnu"
    "desktop-macos|personal-hopspot/desktop/Cargo.toml|aarch64-apple-darwin"
    "desktop-windows|personal-hopspot/desktop/Cargo.toml|x86_64-pc-windows-msvc"
    "android|personal-hopspot/mobile/android/rust/Cargo.toml|aarch64-linux-android"
    "ios|personal-hopspot/mobile/ios/rust/Cargo.toml|aarch64-apple-ios"
    "napi-linux|prns-napi/Cargo.toml|x86_64-unknown-linux-gnu"
    "napi-macos|prns-napi/Cargo.toml|aarch64-apple-darwin"
    "napi-windows|prns-napi/Cargo.toml|x86_64-pc-windows-msvc"
    "nrf52840|personal-hopspot/embedded/nrf52840/Cargo.toml|thumbv7em-none-eabihf"
    "esp32-c6|personal-hopspot/embedded/esp32/boards/xiao-esp32-c6/Cargo.toml|riscv32imac-unknown-none-elf"
    "esp32-s3-heltec-e290|personal-hopspot/embedded/esp32/boards/heltec-e290/Cargo.toml|xtensa-esp32s3-none-elf"
    "esp32-s3-heltec-v3|personal-hopspot/embedded/esp32/boards/heltec-v3/Cargo.toml|xtensa-esp32s3-none-elf"
    "esp32-s3-xiao-esp32s3-wio-sx1262|personal-hopspot/embedded/esp32/boards/xiao-esp32s3-wio-sx1262/Cargo.toml|xtensa-esp32s3-none-elf"
    "esp32-s3-heltec|personal-hopspot/embedded/esp32/boards/heltec-v4/Cargo.toml|xtensa-esp32s3-none-elf"
    "esp32-s3-heltec-r8|personal-hopspot/embedded/esp32/boards/heltec-v4-r8/Cargo.toml|xtensa-esp32s3-none-elf"
    "esp32-s3-heltec-wireless-stick-lite-v3|personal-hopspot/embedded/esp32/boards/heltec-wireless-stick-lite-v3/Cargo.toml|xtensa-esp32s3-none-elf"
    "esp32-s3-tbeam|personal-hopspot/embedded/esp32/boards/t-beam-supreme/Cargo.toml|xtensa-esp32s3-none-elf"
    "wasm|prns-wasm/Cargo.toml|wasm32-unknown-unknown"
    "nrf-dfu-browser|prns-nrf-dfu-wasm/Cargo.toml|wasm32-unknown-unknown"
    "website-rust|docs/website/Cargo.toml|wasm32-unknown-unknown"
    "flasher-macos-arm64|personal-hopspot/flasher/Cargo.toml|aarch64-apple-darwin"
    "flasher-macos-x64|personal-hopspot/flasher/Cargo.toml|x86_64-apple-darwin"
    "flasher-linux-x64|personal-hopspot/flasher/Cargo.toml|x86_64-unknown-linux-gnu"
    "flasher-linux-arm64|personal-hopspot/flasher/Cargo.toml|aarch64-unknown-linux-gnu"
    "flasher-windows-x64|personal-hopspot/flasher/Cargo.toml|x86_64-pc-windows-msvc"
    "host-runtime-connections-linux|prns-host/impls/runtime/Cargo.toml|x86_64-unknown-linux-gnu|connections"
    "host-connections-linux|prns-host/bindings/uniffi/image/Cargo.toml|x86_64-unknown-linux-gnu|connections"
    "host-connections-macos|prns-host/bindings/uniffi/image/Cargo.toml|aarch64-apple-darwin|connections"
    "host-connections-android|prns-host/bindings/uniffi/image/Cargo.toml|aarch64-linux-android|connections"
    "host-connections-ios|prns-host/bindings/uniffi/image/Cargo.toml|aarch64-apple-ios|connections"
    "host-runtime-linux|prns-host/impls/runtime/Cargo.toml|x86_64-unknown-linux-gnu"
    "expo-host-linux|prns-host/bindings/uniffi/image/Cargo.toml|x86_64-unknown-linux-gnu"
    "expo-android|prns-host/bindings/uniffi/image/Cargo.toml|aarch64-linux-android"
    "expo-ios|prns-host/bindings/uniffi/image/Cargo.toml|aarch64-apple-ios"
)

for graph in "${graphs[@]}"; do
    IFS='|' read -r name manifest target features <<<"$graph"
    deny_args=(
        --manifest-path "$root/$manifest"
        --target "$target"
        --locked
        --exclude-dev
    )
    if [[ -n "$features" ]]; then
        deny_args+=(--features "$features")
        echo "[dependency-audit] $name ($target, default features plus $features)"
    else
        echo "[dependency-audit] $name ($target)"
    fi
    cargo deny \
        "${deny_args[@]}" \
        check \
        --config "$root/deny.toml" \
        advisories licenses sources bans
done

python3 "$root/validation/security/unsafe-audit.py"
python3 "$root/validation/security/npm-production-audit.py"
npm --prefix "$root/docs/website" ci --ignore-scripts --no-audit --no-fund
"$root/tools/prns" repo notices generate

echo "RELEASE_DEPENDENCY_AUDIT_COMPLETE"
