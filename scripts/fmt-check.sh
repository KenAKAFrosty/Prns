#!/usr/bin/env bash
# Hygiene gate: the tree stays `cargo fmt`-clean and its intra-doc links resolve.
#
# fmt is checked across the root workspace and EVERY standalone workspace — each is its own
# workspace (own target/toolchain), so `--all` from the root doesn't reach them. The Hopspot
# core crate is a root member, so the root check covers it. personal-hopspot/embedded/esp32
# rides the Xtensa "esp" toolchain (espup), so it is checked only where that toolchain is
# installed (locally / a device runner) and skipped on stock runners. The doc step builds
# personal-rns's docs with private items;
# `#![deny(rustdoc::broken_intra_doc_links)]` turns any broken link into a hard error here.
# Pure checks: nothing is rewritten.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[fmt] root workspace"
cargo fmt --all -- --check

for ws in \
  prns-config \
  prnsd \
  prns-ffi \
  prns-wasm \
  prns-interfaces/tokio \
  prns-interfaces/embassy \
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
