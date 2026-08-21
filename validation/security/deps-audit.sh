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

graphs=(
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
    "esp32-s3-heltec|personal-hopspot/embedded/esp32/boards/heltec-v4/Cargo.toml|xtensa-esp32s3-none-elf"
    "esp32-s3-heltec-r8|personal-hopspot/embedded/esp32/boards/heltec-v4-r8/Cargo.toml|xtensa-esp32s3-none-elf"
    "esp32-s3-tbeam|personal-hopspot/embedded/esp32/boards/t-beam-supreme/Cargo.toml|xtensa-esp32s3-none-elf"
    "wasm|prns-wasm/Cargo.toml|wasm32-unknown-unknown"
    "nrf-dfu-browser|prns-nrf-dfu-wasm/Cargo.toml|wasm32-unknown-unknown"
    "website-rust|docs/website/Cargo.toml|wasm32-unknown-unknown"
    "flasher-macos-arm64|personal-hopspot/flasher/Cargo.toml|aarch64-apple-darwin"
    "flasher-macos-x64|personal-hopspot/flasher/Cargo.toml|x86_64-apple-darwin"
    "flasher-linux-x64|personal-hopspot/flasher/Cargo.toml|x86_64-unknown-linux-gnu"
    "flasher-linux-arm64|personal-hopspot/flasher/Cargo.toml|aarch64-unknown-linux-gnu"
    "flasher-windows-x64|personal-hopspot/flasher/Cargo.toml|x86_64-pc-windows-msvc"
)

for graph in "${graphs[@]}"; do
    IFS='|' read -r name manifest target <<<"$graph"
    echo "[dependency-audit] $name ($target)"
    cargo deny \
        --manifest-path "$root/$manifest" \
        --target "$target" \
        --locked \
        --exclude-dev \
        check \
        --config "$root/deny.toml" \
        advisories licenses sources bans
done

python3 "$root/validation/security/unsafe-audit.py"
python3 "$root/validation/security/npm-production-audit.py"
npm --prefix "$root/docs/website" ci --ignore-scripts --no-audit --no-fund
"$root/tools/prns" repo notices generate

echo "RELEASE_DEPENDENCY_AUDIT_COMPLETE"
