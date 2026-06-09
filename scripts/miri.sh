#!/usr/bin/env bash
# Miri: runs the personal-rns test suite under an interpreter that detects undefined
# behaviour (out-of-bounds, use-after-free, invalid values, strict-provenance
# violations, data races, leaks). Requires the nightly `miri` component.
#
# Honest ROI note: the engine is `#![forbid(unsafe_code)]`, so there is no in-crate
# `unsafe` for Miri to find UB in. Its value here is (1) validating that our *usage*
# of unsafe-internally dependencies (heapless, the dalek/RustCrypto stack) is sound,
# and (2) being ready the moment `unsafe` does appear at a host or FFI boundary.
# For an unsafe-free pure engine this is cheap insurance, not a primary gate. Some
# crypto deps are slow under Miri; scope to one test with `scripts/miri.sh <filter>`.
set -euo pipefail
cd "$(dirname "$0")/.."

rustup toolchain install nightly --component miri --profile minimal 2>/dev/null || true

# Isolation stays ON: the engine injects clock + entropy as data (never reads them
# from the host), so Miri needs no escape hatch and runs deterministically.
# Tree Borrows, NOT miri's default Stacked Borrows. The RustCrypto cipher stack
# (`inout`/`aes`/`cbc`) does in-place input/output pointer aliasing that the
# *experimental* Stacked Borrows model over-rejects — a known false positive that
# would fail every crypto test. Tree Borrows is the newer model the ecosystem
# targets and accepts it. Remove `-Zmiri-tree-borrows` to run strict Stacked Borrows.
echo "[miri] personal-rns (Tree Borrows)"
MIRIFLAGS="${MIRIFLAGS:-} -Zmiri-tree-borrows" cargo +nightly miri test -p personal-rns "$@"

echo "MIRI_GATE_OK"
