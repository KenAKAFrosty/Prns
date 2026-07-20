#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE="$ROOT/personal-hopspot/embedded/nrf52840"
BIN_NAME="t-echo"
BASE="0x27000"
FAMILY="0xADA52840"
VOLUME="/Volumes/TECHOBOOT"

cd "$CRATE"
cargo build --release

HOST_TRIPLE="$(rustc -vV | sed -n 's/host: //p')"
OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$HOST_TRIPLE/bin/llvm-objcopy"
ELF="$CRATE/target/thumbv7em-none-eabihf/release/$BIN_NAME"
BIN="$CRATE/target/$BIN_NAME.bin"
UF2="$CRATE/target/$BIN_NAME.uf2"

"$OBJCOPY" -O binary "$ELF" "$BIN"
python3 "$ROOT/scripts/bin2uf2.py" "$BIN" "$UF2" "$BASE" "$FAMILY"

if [ -d "$VOLUME" ]; then
    cp "$UF2" "$VOLUME/"
    echo "flashed: copied $UF2 to $VOLUME (the T-Echo reboots into the new firmware)"
else
    echo "built: $UF2"
    echo "double-tap RESET on the T-Echo so $VOLUME mounts, then re-run this script (or drag the .uf2 onto the drive)"
fi
