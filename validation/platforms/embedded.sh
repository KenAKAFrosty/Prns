#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

bash validation/platforms/no-std-esp-build.sh
cargo build \
    --manifest-path prns-interfaces/impls/embassy/Cargo.toml \
    --locked \
    --target riscv32imac-unknown-none-elf \
    --features "tcp,wifi-auto,lora,esp-now,bluetooth-auto,usb"
cargo build \
    --manifest-path prns-interfaces/impls/embassy/Cargo.toml \
    --locked \
    --target thumbv7em-none-eabihf \
    --features "lora,bluetooth-auto,usb"
(
    cd personal-hopspot/embedded/nrf52840
    cargo build --release --locked
    cargo build --release --locked --no-default-features --features board-t114 --bin heltec-t114
)

echo "EMBEDDED_BUILD_GATE_OK"
