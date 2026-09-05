#!/usr/bin/env bash
# Compile every shipping ESP face and cataloged qualification target on its real
# architecture without paying the release lane's artifact assembly and
# reproducibility cost. This is the PR forcing function for target-gated S3/C6
# code that a host build cannot see.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

./tools/prns release toolchain esp verify

(
    cd personal-hopspot/embedded/esp32

    for package in \
        hopspot-heltec-e290 \
        hopspot-heltec-v4 \
        hopspot-heltec-v4-r8 \
        hopspot-heltec-wireless-stick-lite-v3 \
        hopspot-t-beam-supreme; do
        cargo +esp check --release --locked \
            -p "$package" \
            --target xtensa-esp32s3-none-elf \
            -Zbuild-std=core,alloc
    done

    cargo +esp check --release --locked \
        -p hopspot-xiao-esp32-c6 \
        --target riscv32imac-unknown-none-elf \
        -Zbuild-std=core,alloc
)

echo "ESP32_FIRMWARE_CHECK_GATE_OK"
