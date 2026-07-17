#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[fmt] root workspace"
cargo fmt --all -- --check

for ws in \
  prns-config \
  prnsd \
  prns-ffi \
  prns-wasm \
  prns-runtime/impls/tokio \
  prns-runtime/impls/embassy \
  prns-interfaces/impls/tokio \
  prns-interfaces/impls/embassy \
  prns-integration-tests \
  personal-hopspot/desktop \
  personal-hopspot/embedded/nrf52840 \
  personal-hopspot/mobile/android/rust \
  personal-hopspot/mobile/ios/rust; do
  echo "[fmt] ${ws}"
  (cd "${ws}" && cargo fmt --all -- --check)
done

if rustup toolchain list | grep -q '^esp'; then
  echo "[fmt] personal-hopspot/embedded/esp32"
  (cd personal-hopspot/embedded/esp32 && cargo fmt --all -- --check)
else
  echo "[fmt] personal-hopspot/embedded/esp32 — SKIPPED (esp toolchain not installed)"
fi

echo "[docs] intra-doc links (personal-rns)"
cargo doc -p personal-rns --no-deps --document-private-items --quiet

echo "FMT_DOC_CHECK_GATE_OK"
