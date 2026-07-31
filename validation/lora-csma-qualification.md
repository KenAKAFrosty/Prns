# Airtime-Quantum LoRa CSMA/CA Qualification

Measured on 2026-07-31 against `d71ac9a8` (`c03af0c9` changes only a host-side
test and is release-binary equivalent). The candidate is the uncommitted
airtime-quantum scheduler change on `trunk`.

## Linked memory and artifact size

| Target | Section or artifact | Baseline | Candidate | Delta |
|---|---|---:|---:|---:|
| Heltec V4 | `.data` | 30,980 B | 30,980 B | 0 B |
| Heltec V4 | `.data.wifi` | 996 B | 996 B | 0 B |
| Heltec V4 | `.bss` | 244,580 B | 244,580 B | 0 B |
| Heltec V4 | `.stack` | 31,516 B | 31,516 B | 0 B |
| Heltec V4 | `.dram2_uninit` | 44,032 B | 44,032 B | 0 B |
| Heltec V4 | `.text` | 1,282,413 B | 1,285,777 B | +3,364 B |
| Heltec V4 | `.rodata` | 777,828 B | 778,900 B | +1,072 B |
| Heltec V4 | application binary | 2,185,728 B | 2,190,176 B | +4,448 B |
| T-Echo | `.data` | 18,044 B | 18,044 B | 0 B |
| T-Echo | `.bss` | 115,552 B | 115,552 B | 0 B |
| T-Echo | `.text` | 415,236 B | 416,780 B | +1,544 B |
| T-Echo | `.rodata` | 53,928 B | 53,936 B | +8 B |
| T-Echo | UF2 | 975,360 B | 978,432 B | +3,072 B |

The reserved-RAM delta is zero on both shipping boards, below the 128-byte
acceptance ceiling. The scheduler adds no heap allocation or packet-sized
storage; the linked growth is code and read-only data only.

## Software evidence

- `cargo test --manifest-path prns-interfaces/impls/embassy/Cargo.toml --features lora --lib`
  passes 46 tests, including the deterministic state machine and corrected
  RNode/PRNS simulation matrix.
- `cargo clippy --manifest-path prns-interfaces/impls/embassy/Cargo.toml --features lora --all-targets -- -D warnings`
  passes.
- The Embassy interface crate cross-builds for
  `riscv32imac-unknown-none-elf` and `thumbv7em-none-eabihf` with the shipping
  feature sets.
- `release.firmware.build` produces final Heltec V4 and T-Echo shipping
  artifacts.

## Hardware qualification

Not yet run. No serial or USB radios were attached during this qualification,
so bidirectional PRNS/RNode framing, measured over-the-air burst duration,
airtime shares, collision behavior, and physical queue drainage still require
two PRNS radios and one matching-profile RNode.
