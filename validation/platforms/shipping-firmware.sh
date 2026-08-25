#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
artifact_root="${PRNS_VALIDATION_ARTIFACTS:-$root/validation-artifacts}"
out_root="$artifact_root/shipping-firmware"

"$root/tools/prns" release firmware build -- --all "$out_root"

echo "SHIPPING_FIRMWARE_GATE_OK"
