#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
output="$root/target/hopspot-mesh-tower-v2"
resource_work="$root/target/flash-artifacts/resources/configured/work/mesh-tower-v2"
elf="$resource_work/cargo/thumbv7em-none-eabihf/release/heltec-mesh-tower-v2"
resource_binary="$resource_work/firmware.bin"
binary="$output/heltec-mesh-tower-v2.bin"
uf2="$output/heltec-mesh-tower-v2.uf2"
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
"$root/tools/prns" build embedded resources report --target mesh-tower-v2

application_base=""
while read -r section_index section_name section_size section_vma section_rest; do
    if [[ "$section_name" == ".vector_table" ]]; then
        application_base="0x$section_vma"
    fi
done < <("$llvm_objdump" -h "$elf")

if [[ -z "$application_base" ]]; then
    echo "the MeshTower V2 ELF does not contain .vector_table" >&2
    exit 1
fi

cp "$resource_binary" "$binary"
python3 "$root/tools/device/bin2uf2.py" \
    "$binary" \
    "$uf2" \
    "$application_base" \
    "$nrf52840_uf2_family"
printf 'MeshTower V2 developer UF2: %s\n' "$uf2"
