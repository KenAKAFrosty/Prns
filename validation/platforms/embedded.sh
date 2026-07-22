#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

bash scripts/no-std-esp-build.sh
cargo build \
    --manifest-path prns-interfaces/impls/embassy/Cargo.toml \
    --locked \
    --target riscv32imac-unknown-none-elf \
    --features "tcp,wifi,lora,esp-now,ble,usb"
cargo build \
    --manifest-path prns-interfaces/impls/embassy/Cargo.toml \
    --locked \
    --target thumbv7em-none-eabihf \
    --features "lora,ble,usb-device"
cargo build \
    --manifest-path personal-hopspot/embedded/nrf52840/Cargo.toml \
    --release \
    --locked

echo "EMBEDDED_BUILD_GATE_OK"
