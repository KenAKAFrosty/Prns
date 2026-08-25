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
    cargo build --release --locked --no-default-features \
        --features board-t-echo,softdevice-s140-v6 \
        --target-dir target/s140-v6
    cargo build --release --locked --no-default-features \
        --features board-t-echo,softdevice-s140-v7 \
        --target-dir target/s140-v7
)
./tools/prns build hopspot t1000e
./tools/prns build hopspot t096
./tools/prns build hopspot t114
./tools/prns build hopspot mesh-tower-v2

echo "EMBEDDED_BUILD_GATE_OK"
