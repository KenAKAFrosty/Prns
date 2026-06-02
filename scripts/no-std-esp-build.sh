#!/usr/bin/env bash
# Forcing-function gate: the Personal Reticulum core must build for the embedded
# substrate (no_std, with and without alloc) AND cross-compile to the ESP32-C6
# (riscv32imac) target. Run this every step so std/alloc creep is caught while
# the surface is smallest. Scope is the core (personal-rns) plus the shared
# Hopspot UI renderer (personal-hopspot-ui) — personal-rnsd is the std-host body,
# and a dedicated embedded body crate is a later chunk.
set -euo pipefail
cd "$(dirname "$0")/.."

C6_TARGET=riscv32imac-unknown-none-elf

echo "[1/9] core: pure no_std (host)"
cargo build -p personal-rns --no-default-features

echo "[2/9] core: no_std + alloc (host)"
cargo build -p personal-rns --no-default-features --features alloc

echo "[3/9] core: pure no_std (ESP32-C6 / ${C6_TARGET})"
cargo build -p personal-rns --no-default-features --target "${C6_TARGET}"

echo "[4/9] core: no_std + alloc (ESP32-C6 / ${C6_TARGET})"
cargo build -p personal-rns --no-default-features --features alloc --target "${C6_TARGET}"

# The embassy contract seam: just embassy-sync + embassy-time (no embassy-net), so
# it compile-checks on the host toolchain. The full embassy-host stack still needs
# the ESP cross-build (heltec), but this keeps the seam itself honest every step.
echo "[5/9] embassy contract seam (no_std, host compile-check)"
cargo build -p personal-rns --no-default-features --features embassy-seam

# The embassy contract runtime a USB-only board (ESP32-C6) builds: the serial
# `serve` shell + `EmbassyContractHost`, no embassy-net/LoRa. Host compile-check
# first (fast), then the real C6 cross-compile the on-board binary depends on.
echo "[6/9] embassy contract runtime (no_std, host compile-check)"
cargo build -p personal-rns --no-default-features --features embassy-contract

echo "[7/9] embassy contract runtime (ESP32-C6 / ${C6_TARGET})"
cargo build -p personal-rns --no-default-features --features embassy-contract --target "${C6_TARGET}"

# The shared Hopspot screen renderer is consumed by the S3 firmware (Xtensa), so
# it must stay no_std. The real Xtensa proof is the heltec build (not in this
# gate); these two cheap builds catch std creep on the host + a riscv cross.
echo "[8/9] hopspot UI: shared renderer (host, no_std)"
cargo build -p personal-hopspot-ui

echo "[9/9] hopspot UI: shared renderer (ESP32-C6 / ${C6_TARGET})"
cargo build -p personal-hopspot-ui --target "${C6_TARGET}"

echo "NO_STD_ESP_BUILD_GATE_OK"
