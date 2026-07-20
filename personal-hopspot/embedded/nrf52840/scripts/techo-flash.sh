#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$HERE"

BASE=0x27000
FAMILY=0xADA52840
ELF=target/thumbv7em-none-eabihf/release/t-echo
BIN=/tmp/t-echo.bin
UF2=/tmp/t-echo.uf2

cargo build --release

rust-objcopy -O binary "$ELF" "$BIN"
python3 "$HERE/scripts/uf2conv.py" "$BIN" --base "$BASE" --family "$FAMILY" --output "$UF2"

MOUNT="$(lsblk -o LABEL,MOUNTPOINT -nr | awk '$1=="TECHOBOOT"{print $2}')"
if [ -z "$MOUNT" ]; then
    echo "TECHOBOOT not mounted — double-tap the T-Echo reset to enter the UF2 bootloader." >&2
    exit 1
fi
cp "$UF2" "$MOUNT/"
sync
echo "flashed $UF2 -> $MOUNT"
