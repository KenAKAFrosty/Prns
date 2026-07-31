# Personal RNS

This crate is one package in the Personal RNS public Rust graph. The complete
feature guide, API documentation, examples, and cross-language SDK overview are
maintained at [reticulum.rs](https://reticulum.rs) and in the
[source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual
MIT/Apache-2.0 license.

## Embedded LoRa spectrum access

`LoRaInterfaceInput` requires an `AirtimePolicy`, a `LoRaSpectrumStatus`, and
`LORA_TX_QUEUE_BYTES` of caller-owned transmit queue storage. Construction
validates frequency, transmit power, preamble, and any fixed airtime limit
before the radio task can start.
`AirtimePolicy::Regional` is the normal choice; a fixed policy may tighten a
regional limit but cannot weaken one.

The SX126x implementation uses continuous receive, preamble/header IRQ
evidence, an adaptive RSSI noise floor, DIFS, randomized contention windows,
and a final IRQ-plus-RSSI check immediately before transmit. Backoff freezes
while the channel is busy. A pending frame expires with an explicit
contention- or duty-limit disposition; it is never forced onto a busy channel.
Split Reticulum packets remain contiguous on air for RNode interoperability.

`LoRaSpectrumStatus::snapshot()` exposes sampled channel occupancy, noise and
CCA levels, deferrals, false preambles, contention and duty drops, and radio
recoveries. These diagnostics are observational; they do not provide a
listen-before-talk bypass.

The active packet remains separate from the packed 6 KiB FIFO, and its
contention timeout begins only when it becomes active. A one-slot manifold lane
provides the ingress handoff. ESP32-S3 Hopspots place the FIFO in PSRAM, while
T-Echo supplies static SRAM storage.
