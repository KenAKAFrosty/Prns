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

## Current-main T114 qualification

A second hardware receipt ran on 2026-08-13 against the clean T114 port at
`78868c5b`, with the shared SX126x receive-reentry correction at `cfe48877` and
upstream `df05c6bf` as their base.

- `cargo build --release --locked --no-default-features --features board-t114
  --bin heltec-t114` completed for `thumbv7em-none-eabihf`. The resulting UF2
  used application base `0x26000`, nRF52840 family ID `0xada52840`, measured
  694,272 bytes, and had SHA-256
  `f1901becf0b2cb19162180425d5e555a5d67562898743314afc6798333147c2f`.
- The image booted on the owner-confirmed Heltec Mesh Node T114 Rev. 2.x.
  Production USB enumerated as `1209:0001`, product
  `Personal Hopspot (Heltec T114)`, serial `PERSONAL-RNS-T114-HOP`, and completed
  USB Auto `Hello`/`HelloAck` with node tag `t114-usb`.
- The independent oracle was a Heltec LoRa32 V4 850-950 MHz PA variant running
  stock RNode 1.86. `rnodeconf` validated the device signature, EEPROM checksum,
  SX1262 identity, firmware version, and host-controlled mode. Stock RNS 1.4.2
  configured it at 915 MHz, 250 kHz bandwidth, SF9, CR4:5, and 7 dBm; the T114
  used the matching channel with its 18-symbol preamble and RNode sync word.
- V4 to T114: stock RNS emitted a fresh signed announce for destination
  `84125f42bd0410207657440faa510c82`. The T114 delivered a 230-byte RF packet
  over USB whose header carried that exact destination hash.
- T114 to V4: a fresh 215-byte stock-RNS signed announce was injected over the
  T114 USB lane. Stock RNS received it through the V4, validated and dispatched
  destination `9e7cecbeb3fefff82314aa0d1601d9d4`, and delivered the exact application
  data `stock-rns-1.4.2-via-prns-v4-to-current-main-t114`.
- After the receipt, the V4 was restored through the signed Prns Hopspot 0.3.4
  package. The helper verified its bootloader, partition table, and application
  (`c029b78248c3bd05bde79b82e31160736cddb7f8e1d88ea6b0fdf374c39762b9`),
  preserved the package's declared provisioning range, and the restored device
  completed USB Auto `Hello`/`HelloAck` with node tag `heltecv4`.

This is a normal-mode, bidirectional, single-frame RNode interoperability
receipt for the exact current-main T114 image. It does not repeat the older
maximum-fragment qualification, measure contention fairness, or claim
multi-node queue drainage.
