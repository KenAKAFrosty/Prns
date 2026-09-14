# Seeed SenseCAP Solar Node P1-Pro Qualification

Observed on 2026-09-14 against commit `73139445`, using the exact
`target/hopspot-sensecap-solar-node/sensecap-solar-node.uf2` produced by
`./tools/prns build hopspot sensecap-solar-node`. The device was a stock
SenseCAP Solar Node P1-Pro that shipped with, and was running, MeshCore
immediately before the observation.

The Solar Node is a carrier for two Seeed modules: a XIAO nRF52840 Plus and a
Wio-SX1262, plus a XIAO L76K GNSS on the Pro variant. It therefore reuses the
nRF52840 and SX1262 already represented in the embedded stack, but needs its own
board authority for pins, the GPIO receive-path enable that accompanies DIO2 RF
switching, GNSS power gating through its load switch, and the S140 7.3.0 flash
geometry. The board reuses the T1000-E's proven flash layout because it carries
the same bootloader, the same resident S140 7.3.0, and the same `0xF4000`
bootloader offset.

```text
UF2 Bootloader 0.9.2-OTAFIX2.2-BP1.3 lib/nrfx (v2.0.0) lib/tinyusb (0.12.0-145-g9775e7691) lib/uf2 (remotes/origin/configupdate-9-gadbb8c7)
Model: Seeed Solar Node P1
Board-ID: nRF52840-SeeedSenseCAPSolarP1-v1
Date: Apr 13 2026
SoftDevice: S140 7.3.0
```

The bootloader mounts as volume `SENSECAP` and its application USB identity is
`2886:0059`. Both differ from the generic Seeed XIAO nRF52840 values published
upstream, so a catalog entry must take them from this device rather than from
the XIAO bootloader definition.

## Artifact

| | |
| --- | --- |
| Raw image | 488,012 bytes, SHA-256 `bd3c5bde0da8b2a51d96f682f994ed5b4fa9829f858e8792e5e9a77c9e2e1ca0` |
| UF2 | 976,384 bytes, 1,907 blocks, SHA-256 `e21adc5f41677973e479ef23fe89ef8801d6a581b2781acec3ff229955619749` |
| Application base | `0x00027000`, family ID `0xADA52840` |
| Region | `0x00027000`–`0x000E9000` (`APPLICATION_FLASH_BYTES` = `0xC2000`) |
| Payload end | `0x0009E300`, leaving 306,432 bytes (38.6%) of application flash free |
| RAM | `0x20010000`–`0x20040000`, 192 KiB |
| Sections | `.vector_table` at `0x00027000`, `.text` `0x68308`, `.rodata` `0xE084` |

The build asserts the ELF vector table equals the linker's
`APPLICATION_FLASH_ORIGIN`; both read `0x00027000`.

## Boot, USB, and recovery

The unmodified `0x27000` image booted. The mesh LED on `P0.19` produced the
expected heartbeat, 100 ms illuminated on the shared 15-second cycle. Windows
enumerated the application as `Personal Hopspot (SenseCAP Solar Node)`, USB
`1209:0001`, serial `PERSONAL-RNS-SOLARNODE-HOP`, status OK.

The native USB Auto host opened the vendor-class interface
(`interface=0 in=0x81 out=0x01`) and held a Connected session across the
observation. Because identity bootstrap and the flash vault run before USB is
brought up, enumeration is itself evidence that entropy seeding and the identity
vault completed.

Recovery continued to mount `SENSECAP` on double reset. The bootloader is never
written by this path, and the factory MeshCore application was captured from
`CURRENT.UF2` beforehand (3,728 blocks spanning `0x1000`–`0xEA000`, family
`0x28860044`) and is restorable by copying it back.

Note for any future catalog work: this bootloader accepts both
`CFG_UF2_BOARD_APP_ID` (`0x28860044`, its own VID/PID pair) and
`CFG_UF2_FAMILY_APP_ID` (`0xADA52840`). The repository's generic nRF52840 family
ID is therefore correct for this board.

## Bidirectional LoRa result

The peer was a Heltec V4 running current RNode firmware on a Raspberry Pi 3B
with stock Reticulum (Python RNS 1.5.4), configured to match the firmware's
`DEFAULT_868_PROFILE`: 868.300 MHz, SF9, 250 kHz bandwidth, coding rate 4/5. The
peer independently reported a 3.52 kbps link rate, which is the exact bitrate
those parameters produce, and a −95 dBm noise floor with no interference before
the test.

Transmit was confirmed first. A host-originated path request crossing the USB
Auto session produced RF energy at −66 dBm on the peer, 30 dB above its noise
floor, and the peer decoded 67 bytes as a valid Reticulum packet. Stock RNS
demodulating our frames establishes that the sync word, preamble and modulation
are RNode-compatible in practice and not only by construction.

Receive was confirmed with the LAN path removed. With the peer's `AutoInterface`
disabled so LoRa was its only route, and with a freshly started host daemon
holding no cached paths, NomadNet announces originating on the peer were heard
by the Solar Node and relayed to the host. The host learned both the
`nomadnetwork.node` and `lxmf.delivery` destinations at two hops, via the Solar
Node's own transport instance, on the USB Auto interface. That path could not
have been learned over the LAN, so it confirms the receive chain including the
GPIO receive enable and the DIO1 interrupt, and confirms the board forwarding as
a transport node rather than merely terminating traffic.

Counters at the end of the observation were symmetric at the peer, 424 bytes in
each direction, with airtime at 0.03% of the hour and no reported interference
beyond the expected transmissions.

## Compatibility and remaining limits

The firmware profile is fixed at EU 868 MHz, and its transmit power is 14 dBm
because `Region::Eu868` caps there; the hardware's advertised 22 dBm is not
usable in this band. Regional duty cycle is applied by `AirtimePolicy::Regional`
but was not characterised at the limit.

This receipt does not qualify GNSS. The L76K adapter and its load-switch power
gating are implemented and the receiver is enabled at startup, but no position
fix was observed.

Battery sensing and the user button are not wired into the runtime. No headless
board in the tree exposes either, so both need runtime plumbing rather than
board code. Two unresolved hardware details are recorded for whoever does that
work: the battery divider is 1 MΩ/512 kΩ, giving a ratio near 2.95 rather than
the 3.3 published in one reference firmware, and its enable on `P0.14` is active
low; and the vendor block diagram assigns `P1.07` to the user key and `P1.01` to
the power key, while both reference firmwares use `P1.01` as their user button.

This receipt does not qualify contention, collision behaviour, CSMA fairness,
multi-node queue drainage, BLE, Grove I2C, long-run solar or battery behaviour,
or a signed release artifact. The board is not present in
`release/flash/boards.json` and has no flash-manifest entry; it is a developer
UF2 target only, following the MeshTower V2 precedent.

One caveat on the observation platform rather than the board: the Raspberry Pi
hosting the peer was intermittently under-voltage and thermally throttled
throughout, which cost several transport retries on the peer side. It did not
affect the Solar Node, whose USB Auto session and heartbeat were continuous.
