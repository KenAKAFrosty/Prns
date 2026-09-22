#!/usr/bin/env bash
set -euo pipefail

if (( $# != 3 )); then
    echo "usage: hopspot-build-only-uf2.sh <resource-target-id> <firmware-binary-name> <board-display-name>" >&2
    exit 1
fi

target_id="$1"
firmware_name="$2"
board_name="$3"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
output="$root/target/hopspot-$target_id"
resource_work="$root/target/flash-artifacts/resources/configured/work/$target_id"
resource_cache="$root/target/flash-artifacts/resources/configured/cargo-cache"
elf="$resource_cache/thumbv7em-none-eabihf/release/$firmware_name"
resource_binary="$resource_work/firmware.bin"
binary="$output/$firmware_name.bin"
uf2="$output/$firmware_name.uf2"
nrf52840_uf2_family=0xADA52840

rust_sysroot="$(rustc --print sysroot)"
rust_host=""
while IFS= read -r version_line; do
    case "$version_line" in
        "host: "*) rust_host="${version_line#host: }" ;;
    esac
done < <(rustc -vV)

if [[ -z "$rust_host" ]]; then
    echo "could not resolve the Rust host triple" >&2
    exit 1
fi

llvm_tools="$rust_sysroot/lib/rustlib/$rust_host/bin"
llvm_objdump="$llvm_tools/llvm-objdump"
if [[ ! -x "$llvm_objdump" ]]; then
    echo "llvm-tools-preview is required; run: rustup component add llvm-tools-preview" >&2
    exit 1
fi

mkdir -p "$output"
"$root/tools/prns" build embedded resources report --target "$target_id"

application_base=""
while read -r section_index section_name section_size section_vma section_rest; do
    if [[ "$section_name" == ".vector_table" ]]; then
        application_base="0x$section_vma"
    fi
done < <("$llvm_objdump" -h "$elf")

if [[ -z "$application_base" ]]; then
    printf 'the %s ELF does not contain .vector_table\n' "$board_name" >&2
    exit 1
fi

cp "$resource_binary" "$binary"
python3 "$root/tools/device/bin2uf2.py" \
    "$binary" \
    "$uf2" \
    "$application_base" \
    "$nrf52840_uf2_family"
printf '%s developer UF2: %s\n' "$board_name" "$uf2"
