#!/usr/bin/env bash
# Miri: runs the personal-rns test suite under an interpreter that detects undefined
# behaviour (out-of-bounds, use-after-free, invalid values, strict-provenance
# violations, data races, leaks). Requires the nightly `miri` component.
#
# Honest ROI note: the engine is `#![forbid(unsafe_code)]`, so there is no in-crate
# `unsafe` for Miri to find UB in. Its value here is (1) validating that our *usage*
# of unsafe-internally dependencies (heapless, the dalek/RustCrypto stack) is sound,
# and (2) being ready the moment `unsafe` does appear — e.g. in personal-rns-ffi.
# For an unsafe-free pure engine this is cheap insurance, not a primary gate. Some
# crypto deps are slow under Miri; scope to one test with `scripts/miri.sh <filter>`.
set -euo pipefail
cd "$(dirname "$0")/.."

rustup toolchain install nightly --component miri --profile minimal 2>/dev/null || true

# Isolation stays ON: the engine injects clock + entropy as data (never reads them
# from the host), so Miri needs no escape hatch and runs deterministically.
echo "[miri] personal-rns"
cargo +nightly miri test -p personal-rns "$@"

echo "MIRI_GATE_OK"
