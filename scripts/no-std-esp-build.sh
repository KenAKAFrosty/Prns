#!/usr/bin/env bash
# Forcing-function gate: the Personal Reticulum core must build for the embedded
# substrate (no_std, with and without alloc) AND cross-compile to the ESP32-C6
# (riscv32imac) target. Run this every step so std/alloc creep is caught while
# the surface is smallest. Scope is the core (personal-rns) only — personal-rnsd
# is the std-host body, and a dedicated embedded body crate is a later chunk.
set -euo pipefail
cd "$(dirname "$0")/.."

C6_TARGET=riscv32imac-unknown-none-elf

echo "[1/4] core: pure no_std (host)"
cargo build -p personal-rns --no-default-features

echo "[2/4] core: no_std + alloc (host)"
cargo build -p personal-rns --no-default-features --features alloc

echo "[3/4] core: pure no_std (ESP32-C6 / ${C6_TARGET})"
cargo build -p personal-rns --no-default-features --target "${C6_TARGET}"

echo "[4/4] core: no_std + alloc (ESP32-C6 / ${C6_TARGET})"
cargo build -p personal-rns --no-default-features --features alloc --target "${C6_TARGET}"

echo "NO_STD_ESP_BUILD_GATE_OK"
