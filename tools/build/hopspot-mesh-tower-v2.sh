#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
crate="$root/personal-hopspot/embedded/nrf52840"
# Optional lab override: PRNS_LORA_PROFILE=montreal for the Montreal LoRa mesh channel.
lora_profile="${PRNS_LORA_PROFILE:-}"
artifact_suffix=""
if [[ -n "$lora_profile" && "$lora_profile" != "default" ]]; then
    artifact_suffix="-${lora_profile//[^A-Za-z0-9._-]/_}"
fi
output="$root/target/hopspot-mesh-tower-v2${artifact_suffix}"
cargo_target="$output/cargo"
elf="$cargo_target/thumbv7em-none-eabihf/release/heltec-mesh-tower-v2"
binary="$output/heltec-mesh-tower-v2${artifact_suffix}.bin"
uf2="$output/heltec-mesh-tower-v2${artifact_suffix}.uf2"
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
llvm_objcopy="$llvm_tools/llvm-objcopy"
llvm_objdump="$llvm_tools/llvm-objdump"
if [[ ! -x "$llvm_objcopy" || ! -x "$llvm_objdump" ]]; then
    echo "llvm-tools-preview is required; run: rustup component add llvm-tools-preview" >&2
    exit 1
fi

mkdir -p "$output"
(
    cd "$crate"
    if [[ -n "$lora_profile" ]]; then
        export PRNS_LORA_PROFILE="$lora_profile"
        printf 'MeshTower V2 LoRa profile: %s\n' "$lora_profile"
    else
        unset PRNS_LORA_PROFILE
    fi
    cargo build --release --locked --no-default-features \
        --features board-mesh-tower-v2,softdevice-s140-v6 \
        --bin heltec-mesh-tower-v2 \
        --target-dir "$cargo_target"
)

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

"$llvm_objcopy" -O binary "$elf" "$binary"
python3 "$root/tools/device/bin2uf2.py" \
    "$binary" \
    "$uf2" \
    "$application_base" \
    "$nrf52840_uf2_family"
printf 'MeshTower V2 developer UF2: %s\n' "$uf2"
