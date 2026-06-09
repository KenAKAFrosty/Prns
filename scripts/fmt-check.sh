#!/usr/bin/env bash
# Hygiene gate: the tree stays `cargo fmt`-clean and its intra-doc links resolve.
#
# fmt is checked across the root workspace and the standalone Hopspot app
# workspace separately — it is its own workspace (own target/toolchain), so
# `--all` from the root doesn't reach it. The Hopspot UI crate is a root member,
# so the root check covers it. The doc step
# builds personal-rns's docs with
# private items; `#![deny(rustdoc::broken_intra_doc_links)]` turns any broken
# link into a hard error here. Pure checks: nothing is rewritten.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[fmt] root workspace"
cargo fmt --all -- --check

for ws in personal-hopspot/app; do
  echo "[fmt] ${ws}"
  (cd "${ws}" && cargo fmt --all -- --check)
done

echo "[docs] intra-doc links (personal-rns)"
cargo doc -p personal-rns --no-deps --document-private-items --quiet

echo "FMT_DOC_CHECK_GATE_OK"
