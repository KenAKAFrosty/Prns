#!/usr/bin/env bash
# Binary-size axis (see benchmarks/CONTRIBUTING.md): "what the engine costs on a
# constrained target." Builds the ESP32-C6 host — a real no_std/no_main firmware that
# drives the engine on riscv32imac — and reports the `.text` breakdown by crate, so
# `personal-rns`'s line is the engine's code cost. Stamped with the commit + toolchain
# so a number reproduces.
#
# Needs the esp build env for esp-hal's build scripts (LIBCLANG). Run:
#   ./scripts/binary-size.sh
set -euo pipefail
cd "$(dirname "$0")/.."

commit="$(git rev-parse --short HEAD 2>/dev/null || echo '?')"
# shellcheck disable=SC1090
source ~/export-esp.sh 2>/dev/null || true

cd hosts/esp32-c6
echo "commit:    ${commit}"
echo "toolchain: $(rustc --version)"
echo "target:    riscv32imac-unknown-none-elf  (ESP32-C6, no_std)"
echo

# `--crates`: per-crate `.text` share. `personal-rns` is the engine; the rest is the
# crypto (sha2/curve25519/aes/ed25519) RNS requires plus the esp-hal/embassy substrate.
cargo bloat --release --crates -n 20
