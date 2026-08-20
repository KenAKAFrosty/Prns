#!/usr/bin/env bash
set -euo pipefail

if (( $# != 1 )); then
    echo "usage: hopspot-nrf52840.sh <t096|t114|t1000e>" >&2
    exit 1
fi

board="$1"
case "$board" in
    t096)
        board_name="T096"
        board_feature="board-t096"
        firmware_name="t096"
        ;;
    t114)
        board_name="T114"
        board_feature="board-t114"
        firmware_name="heltec-t114"
        ;;
    t1000e)
        board_name="T1000-E"
        board_feature="board-t1000e"
        firmware_name="t1000e"
        ;;
    *)
        printf 'unsupported nRF52840 board: %s\n' "$board" >&2
        exit 1
        ;;
esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
crate="$root/personal-hopspot/embedded/nrf52840"
output="$root/target/hopspot-$board"
cargo_target="$output/cargo"
elf="$cargo_target/thumbv7em-none-eabihf/release/$firmware_name"
binary="$output/$firmware_name.bin"
uf2="$output/$firmware_name.uf2"
nrf52840_uf2_family=0xADA52840
uf2_payload_bytes=256

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
    cargo build --release --locked --no-default-features \
        --features "$board_feature" \
        --bin "$firmware_name" \
        --target-dir "$cargo_target"
)

application_base=""
application_flash_origin=""
application_flash_bytes=""
while read -r section_index section_name section_size section_vma section_rest; do
    if [[ "$section_name" == ".vector_table" ]]; then
        application_base="$((16#$section_vma))"
    fi
done < <("$llvm_objdump" -h "$elf")

if [[ -z "$application_base" ]]; then
    printf 'the %s ELF does not contain .vector_table\n' "$board_name" >&2
    exit 1
fi

while read -r symbol_address symbol_binding symbol_type symbol_size symbol_name; do
    case "$symbol_name" in
        APPLICATION_FLASH_ORIGIN)
            application_flash_origin="$((16#$symbol_address))"
            ;;
        APPLICATION_FLASH_BYTES)
            application_flash_bytes="$((16#$symbol_address))"
            ;;
    esac
done < <("$llvm_objdump" -t "$elf")

if [[ -z "$application_flash_origin" || -z "$application_flash_bytes" ]]; then
    printf 'the %s ELF does not expose its application flash bounds\n' "$board_name" >&2
    exit 1
fi

if (( application_base != application_flash_origin )); then
    printf '%s vector table begins at 0x%08x, expected application origin 0x%08x\n' \
        "$board_name" "$application_base" "$application_flash_origin" >&2
    exit 1
fi

"$llvm_objcopy" -O binary "$elf" "$binary"
binary_bytes="$(wc -c < "$binary")"
packaged_bytes="$((
    (binary_bytes + uf2_payload_bytes - 1) / uf2_payload_bytes * uf2_payload_bytes
))"
if (( packaged_bytes > application_flash_bytes )); then
    printf '%s UF2 payload occupies %d bytes, exceeding the %d-byte application region\n' \
        "$board_name" "$packaged_bytes" "$application_flash_bytes" >&2
    exit 1
fi

python3 "$root/tools/device/bin2uf2.py" \
    "$binary" \
    "$uf2" \
    "$(printf '0x%08x' "$application_base")" \
    "$nrf52840_uf2_family"
printf '%s developer UF2: %s\n' "$board_name" "$uf2"
