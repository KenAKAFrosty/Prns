#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

./tools/prns release toolchain esp verify

./tools/prns build embedded resources report --all --platform esp

echo "ESP32_FIRMWARE_CHECK_GATE_OK"
