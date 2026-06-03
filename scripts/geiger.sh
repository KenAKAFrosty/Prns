#!/usr/bin/env bash
# cargo-geiger: counts `unsafe` usage across the *dependency tree*. Because our own
# crates are `forbid(unsafe_code)`, every number this prints is someone else's code —
# use it to (1) know your real transitive unsafe exposure before any "safe Rust"
# marketing claim, and (2) catch a dependency bump that suddenly pulls in a lot of
# unsafe.
#
# Install: `cargo install cargo-geiger`.  Geiger can be finicky on large trees / newer
# resolvers; if it fails to build the graph, fall back to `cargo tree` + a manual look
# at the crates that matter (the crypto + heapless stack).
set -euo pipefail
cd "$(dirname "$0")/.."

# --all-features so the embedded (embassy/LoRa) dependencies are counted too.
# Target the package's own ABSOLUTE manifest path. cargo-geiger bails from the
# virtual workspace manifest ("requires an actual package") AND rejects a relative
# --manifest-path, so build the absolute path from the repo root. Default features
# cover the std + crypto surface (dalek/aes/heapless/getrandom) where the interesting
# inherited unsafe lives and build reliably on the host; append `--features
# embassy-contract` etc. for more. Run with no args: `scripts/geiger.sh`.
cargo geiger --manifest-path "$(pwd)/personal-rns/Cargo.toml" "$@"
