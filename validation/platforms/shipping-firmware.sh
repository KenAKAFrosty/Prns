#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
artifact_root="${PRNS_VALIDATION_ARTIFACTS:-$root/validation-artifacts}"
out_root="$artifact_root/shipping-firmware"

for board in heltec-v4 heltec-v4-r8 t-beam-supreme xiao-esp32-c6 t-echo t114 t096 t1000-e; do
    "$root/tools/prns" release firmware build -- "$board" "$out_root"
done

echo "SHIPPING_FIRMWARE_GATE_OK"
