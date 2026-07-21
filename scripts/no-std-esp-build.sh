#!/usr/bin/env bash
# Forcing-function gate: the Personal Reticulum core must build for the embedded
# substrate (no_std, with and without alloc) AND cross-compile to the ESP32-C6
# (riscv32imac) target. Run this every step so std/alloc creep is caught while the
# surface is smallest. Scope is the umbrella (personal-rns) plus the shared Hopspot
# UI renderer (personal-hopspot-core); the board firmwares prove the full stacks,
# this gate is the cheap every-step guard.
set -euo pipefail
cd "$(dirname "$0")/.."

C6_TARGET=riscv32imac-unknown-none-elf

echo "[1/8] core: pure no_std (host)"
cargo build --locked -p personal-rns --no-default-features

echo "[2/8] core: no_std + alloc (host)"
cargo build --locked -p personal-rns --no-default-features --features alloc

echo "[3/8] core: pure no_std (ESP32-C6 / ${C6_TARGET})"
cargo build --locked -p personal-rns --no-default-features --target "${C6_TARGET}"

echo "[4/8] core: no_std + alloc (ESP32-C6 / ${C6_TARGET})"
cargo build --locked -p personal-rns --no-default-features --features alloc --target "${C6_TARGET}"

# The embassy runtime lane (the embassy bind + reactor over the core seam), no
# embassy-net/LoRa. Host compile-check first (fast), then the real C6 cross-compile
# the on-board binary depends on.
echo "[5/8] embassy host runtime (no_std, host compile-check)"
cargo build --locked -p personal-rns --no-default-features --features embassy-host

echo "[6/8] embassy host runtime (ESP32-C6 / ${C6_TARGET})"
cargo build --locked -p personal-rns --no-default-features --features embassy-host --target "${C6_TARGET}"

# The shared Hopspot screen renderer is consumed by the Heltec V4 firmware (Xtensa), so
# it must stay no_std. The real Xtensa proof is the heltec build (not in this
# gate); these two cheap builds catch std creep on the host + a riscv cross.
echo "[7/8] hopspot UI: shared renderer (host, no_std)"
cargo build --locked -p personal-hopspot-core

echo "[8/8] hopspot UI: shared renderer (ESP32-C6 / ${C6_TARGET})"
cargo build --locked -p personal-hopspot-core --target "${C6_TARGET}"

echo "NO_STD_ESP_BUILD_GATE_OK"
