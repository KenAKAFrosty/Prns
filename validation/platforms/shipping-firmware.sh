#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
artifact_root="${PRNS_VALIDATION_ARTIFACTS:-$root/validation-artifacts}"
out_root="$artifact_root/shipping-firmware"

"$root/tools/prns" release firmware build -- heltec-v4 "$out_root"

for board in heltec-v4-r8 t-beam-supreme xiao-esp32-c6 t-echo; do
    PRNS_EMBEDDED_SITE_READY=1 \
    "$root/tools/prns" release firmware build -- "$board" "$out_root"
done

echo "SHIPPING_FIRMWARE_GATE_OK"
