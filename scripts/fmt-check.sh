#!/usr/bin/env bash
# Hygiene gate: the tree stays `cargo fmt`-clean and its intra-doc links resolve.
#
# fmt is checked across the root workspace and each standalone workspace (the
# device hosts + the Hopspot app) separately — they are their own workspaces
# (own targets/toolchains), so `--all` from the root doesn't reach them. The
# Hopspot UI crate is a root member, so the root check covers it. The doc step
# builds personal-rns's docs with
# private items; `#![deny(rustdoc::broken_intra_doc_links)]` turns any broken
# link into a hard error here. Pure checks: nothing is rewritten.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[fmt] root workspace"
cargo fmt --all -- --check

for ws in hosts/esp32-c6 hosts/heltec-lora32 hosts/nrf52840 personal-hopspot/app; do
  echo "[fmt] ${ws}"
  (cd "${ws}" && cargo fmt --all -- --check)
done

echo "[docs] intra-doc links (personal-rns)"
cargo doc -p personal-rns --no-deps --document-private-items --quiet

echo "FMT_DOC_CHECK_GATE_OK"
