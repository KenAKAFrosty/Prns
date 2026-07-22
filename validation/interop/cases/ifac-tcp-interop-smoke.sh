#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
export PRNS_IFAC_NETWORK_NAME="${PRNS_IFAC_NETWORK_NAME:-prns-interop}"
export PRNS_IFAC_PASSPHRASE="${PRNS_IFAC_PASSPHRASE:-ifac-parity-secret}"
export PRNS_IFAC_SIZE_BYTES="${PRNS_IFAC_SIZE_BYTES:-16}"
exec bash "$ROOT/validation/interop/cases/local-transit-smoke.sh"
