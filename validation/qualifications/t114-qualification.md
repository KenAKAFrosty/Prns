# Heltec Mesh Node T114 Qualification

Observed independently on 2026-08-19 against commit `c40e8cb0`, using the exact
`target/hopspot-t114/heltec-t114.uf2` produced by
`./tools/prns build hopspot t114`. The device was a stock Heltec Mesh Node T114
Rev. 2.x restored to its factory bootloader:

The T114 combines the nRF52840 and SX1262 already represented in the embedded
stack, but it needs its own board authority for pins, external crystal startup,
TCXO and DIO2 RF-switch control, storage ranges, and UF2 memory layout. That
initial target, shared SX126x receive-reentry fix, and hardware qualification
were contributed by [Markik](https://github.com/mark-ik) in
[PR #111](https://github.com/KenAKAFrosty/Prns/pull/111).

```text
UF2 Bootloader 0.9.0-2-g836c8dc-dirty
Model: HT-n5262
Board-ID: HT-n5262
Date: Jul  9 2024
SoftDevice: S140 6.1.1
```

## Boot, USB, and recovery

The unmodified `0x26000` image booted and produced heartbeat LED activity.
Windows enumerated the application as Personal Hopspot (Heltec T114), USB
`1209:0001`, with `ConfigManagerErrorCode` 0, service `WINUSB`, and compatible
ID `USB\MS_COMP_WINUSB`. Before the device-level Microsoft OS descriptor fix,
the same hardware reported Code 28 and had no bound service.

The browser USB Auto host opened the single vendor-class interface with MTU
8192 and completed the USB Auto session. The session carried received radio
announces up to the host and carried host-originated announces down to the
radio. Double-reset recovery continued to mount `HT-n5262`; after restoration
from an S140 7.3 bootloader, the factory bootloader also remained in recovery
when it correctly found no valid `0x26000` application.

## Bidirectional LoRa result

The peer was a known-good RNode at 915.000 MHz, SF9, 250 kHz bandwidth, and
coding rate 4/5. Five independently identified announces heard by that RNode
arrived through the T114 and its USB Auto session in the same second at exactly
one additional hop. Two fresh browser-node destinations crossed the other way,
leaving through the T114 radio and arriving at the RNode at two hops. This
confirms normal-mode, bidirectional LoRa and USB Auto traffic for the exact
image and stock layout.

## Compatibility and remaining limits

The stock `0x26000` UF2 is incompatible with a T114 re-bootloadered for the S140
7.3 `0x27000` application layout. Such a bootloader jumps four KiB above the
image, so a successful UF2 copy is not evidence that the application can boot.

This receipt does not qualify physical contention, collision behavior, CSMA
fairness, multi-node queue drainage, regional configuration, BLE, or a signed
release artifact. The firmware profile is fixed at US 915 MHz. The older
[airtime-quantum CSMA/CA qualification](lora-csma-qualification.md) remains the
authority for its separately recorded hardware status.

The observation predates the shared native USB host move. It proves the T114
vendor-class interface and USB Auto traffic with the browser host; the
post-move `prnsd` WinUSB discovery, reconnect, recovery-cycle rediscovery, and
interface-release checks remain Windows hardware acceptance work.
