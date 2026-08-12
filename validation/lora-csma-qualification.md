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

## Hardware framing qualification

The scheduler qualification above still has no collision or airtime-share
receipt. A separate framing receipt ran on 2026-08-10 for the Heltec Mesh Node
T114 Rev. 2.x port, using the worktree based on `72b6b30d` and the
`prns-t114-hopspot-v24` application:

- T114 production USB enumerated as `1209:0001`, product
  `Personal Hopspot (Heltec T114)`, serial `PERSONAL-RNS-T114-HOP`, and completed
  the USB Auto `Hello`/`HelloAck` exchange with node tag `t114-usb`.
- Two Heltec V4 RNodes running firmware 1.86 supplied the RF oracle at 915 MHz,
  250 kHz bandwidth, SF9, CR4:5, and an 18-symbol preamble. The inspection
  harness explicitly selected promiscuous mode to expose the one-byte RNode air
  header, stripped or reassembled that framing, then handed the packet to stock
  RNS 1.4.2 for signature and announce validation.
- V4 to T114: a 214-byte signed announce forced into 100/114-byte split
  payloads reassembled and crossed USB. A second 354-byte signed announce used
  the canonical 254/100-byte split and crossed USB as one 370-byte transported
  announce.
- T114 to V4: a 358-byte signed announce injected over USB became a 374-byte
  transported announce. The V4 exposed two frames with the same `0x01` header
  and 254/120-byte payloads; stock RNS 1.4.2 reassembled them, validated the
  signature, and delivered the exact 194-byte application data.
- Without rebooting after that split transmission, the T114 received and
  forwarded both a fresh single frame and a fresh canonical 254/100-byte split.
- The T114 also passed single-frame signed announces in both directions. The
  stateful receipt found three shared SX1262 driver defects: packet-parameter
  writes reset the IQ-polarity errata register; this receiver required an
  explicit RX re-arm after `RxDone`; and RX re-entry after transmit had to wait
  for DIO1 to drop after clearing `TxDone`. All corrections live at the shared
  radio boundary rather than in the board module.

The tested UF2 SHA-256 is
`aade7577bf2f6412a8df3700638421b21c52db532dcd7ca8cd19e4690e219990`.
This proves bidirectional single and split RNode air framing, USB carriage, and
stock-RNS packet validation. Because the V4s were used as promiscuous framing
oracles, it does not claim stock RNode normal-mode end to end. Measured
contention fairness, collision behavior, and physical multi-node queue drainage
remain open for the airtime-quantum scheduler.
