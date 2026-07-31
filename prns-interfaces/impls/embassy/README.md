# Personal RNS

This crate is one package in the Personal RNS public Rust graph. The complete
feature guide, API documentation, examples, and cross-language SDK overview are
maintained at [reticulum.rs](https://reticulum.rs) and in the
[source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual
MIT/Apache-2.0 license.

## Embedded LoRa spectrum access

`LoRaInterfaceInput` requires an `AirtimePolicy` and a
`LoRaSpectrumStatus`. Construction validates frequency, transmit power,
preamble, and any fixed airtime limit before the radio task can start.
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

The Heltec V4 release validation build measured this complete stewardship path
at 13,484 additional linked bytes versus the pre-change trunk build, while BSS
decreased by 128 bytes. The retained flash cost covers interrupt-evidence
handling, transactional recovery, adaptive access, explicit outcomes, and
operator diagnostics.

The embedded Hopspot lane holds one packet in active radio custody and three
waiting outbound packets. Only the outbound side is deeper. Compared with the
one-slot lane, the two additional 508-byte payload slots add 1,088 bytes of
static storage on Heltec V4 and reduce its linker-reserved stack from 37,212 to
36,124 bytes; total reserved DRAM is unchanged and the path performs no heap
allocation. The same comparison adds 1,056 bytes of BSS on T-Echo/nRF52840.
