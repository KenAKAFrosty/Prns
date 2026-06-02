#!/usr/bin/env bash
# Forcing-function gate: the Personal Reticulum core must build for the embedded
# substrate (no_std, with and without alloc) AND cross-compile to the ESP32-C6
# (riscv32imac) target. Run this every step so std/alloc creep is caught while
# the surface is smallest. Scope is the core (personal-rns) only — personal-rnsd
# is the std-host body, and a dedicated embedded body crate is a later chunk.
set -euo pipefail
cd "$(dirname "$0")/.."

C6_TARGET=riscv32imac-unknown-none-elf

echo "[1/7] core: pure no_std (host)"
cargo build -p personal-rns --no-default-features

echo "[2/7] core: no_std + alloc (host)"
cargo build -p personal-rns --no-default-features --features alloc

echo "[3/7] core: pure no_std (ESP32-C6 / ${C6_TARGET})"
cargo build -p personal-rns --no-default-features --target "${C6_TARGET}"

echo "[4/7] core: no_std + alloc (ESP32-C6 / ${C6_TARGET})"
cargo build -p personal-rns --no-default-features --features alloc --target "${C6_TARGET}"

# The embassy contract seam: just embassy-sync + embassy-time (no embassy-net), so
# it compile-checks on the host toolchain. The full embassy-host stack still needs
# the ESP cross-build (heltec), but this keeps the seam itself honest every step.
echo "[5/7] embassy contract seam (no_std, host compile-check)"
cargo build -p personal-rns --no-default-features --features embassy-seam

# The embassy contract runtime a USB-only board (ESP32-C6) builds: the serial
# `serve` shell + `EmbassyContractHost`, no embassy-net/LoRa. Host compile-check
# first (fast), then the real C6 cross-compile the on-board binary depends on.
echo "[6/7] embassy contract runtime (no_std, host compile-check)"
cargo build -p personal-rns --no-default-features --features embassy-contract

echo "[7/7] embassy contract runtime (ESP32-C6 / ${C6_TARGET})"
cargo build -p personal-rns --no-default-features --features embassy-contract --target "${C6_TARGET}"

echo "NO_STD_ESP_BUILD_GATE_OK"
