# Heltec Mesh Node T096 Qualification

The later browser-flasher and exact-artifact evidence for the 0.3.7 bring-up is
recorded in the
[T096 and T1000-E developer flasher qualification](flasher-0.3.7-qualification.md).

Two exact developer artifacts were exercised during bring-up. The first
qualified the complete interface and persistence paths. The second is the
current-head regression artifact after integrating `origin/trunk` and the
subsequent bounded-Resource work.

## Current-head regression

Built and flashed on 2026-08-20 from the uncommitted working tree based on
commit `f4c830e9`, using `./tools/prns build hopspot t096`:

```text
SHA-256: 21169b4055a8087f80ada40d63a3c807105d582b8bab4f89ef97184a36a7ce7f
UF2 size: 1,040,896 bytes
UF2 blocks: 2,033
raw application image: 520,252 bytes
application base: 0x00026000
nRF52840 family: 0xADA52840
payload end: 0x000A5100
application region end: 0x000E8000
packaged flash headroom: 274,176 bytes
.data + .bss: 124,772 bytes
application RAM headroom: 80,028 bytes
```

The artifact retained the `0x26000` vector-table address and `0xADA52840` UF2
family. It is 4,096 bytes (eight UF2 blocks) larger than the original
qualification artifact after the intervening Resource-admission work and
upstream integration. Section and address inspection found no layout drift;
the image remains well inside both its application flash and RAM regions.

The original application accepted the WebUSB bootloader request, the current
UF2 booted and enumerated as `Personal Hopspot (Heltec T096)`, and that current
application independently accepted the same WebUSB request before the exact
UF2 was restored a second time. macOS reported an expected copy-finalization
I/O error when the UF2 bootloader disconnected itself after receiving the final
block; each transfer was followed by successful application enumeration.
The board's normal interface screen then remained visible and live, and an
on-device announce advanced its displayed interface traffic counters.

## Original full-path qualification

Observed independently on 2026-08-20 against an uncommitted working tree based
on commit `3e666bc8`, using the then-current `target/hopspot-t096/t096.uf2`:

```text
SHA-256: 2a6c80e396d165ab7f1d81b26c3ed73a08b75551dbfd7bc53655df93f70df2ed
UF2 size: 1,036,800 bytes
UF2 blocks: 2,025
application base: 0x00026000
nRF52840 family: 0xADA52840
payload end: 0x000A4900
```

The device retained its stock Heltec/Adafruit-derived recovery bootloader:

```text
UF2 Bootloader 0.9.0-2-g836c8dc-dirty
Model: HT-n5262G
Board-ID: HT-n5262G
Date: Mar 19 2026
SoftDevice: S140 6.1.1
```

## Boot, face, USB, and recovery

The `0x26000` application booted with a working ST7735 display, backlight,
single-button short/long-press controls, live interface cards, and battery
indicator. The application enumerated on macOS as USB `1209:0001`, manufacturer
`Stay Personal`, product `Personal Hopspot (Heltec T096)`, and serial
`PERSONAL-RNS-T096-HOP`.

A native host opened the WebUSB interface and completed the USB Auto handshake.
It received `HelloAck` from node tag `74 30 39 36 2d 75 73 62` (`t096-usb`). A
device-level vendor request `0x50` with value `0x5052` and index `0x4e53` was
then accepted by the application and entered the stock UF2 bootloader without
a physical double-reset. A single reset returned to the intact application and
the same USB identity.

## Bidirectional LoRa and Bluetooth

The LoRa peer was a Personal Hopspot Heltec V4 connected to a native PRNS host
over USB Auto. Repeated fresh host announces crossed USB to the V4 radio and
were received by the T096, whose live LoRa counters climbed during the run. In
the reverse direction, multiple T096 announces crossed LoRa to the V4 and USB
to the host. The host independently identified destination
`42bd597b70358b5aecb050ce50688ce7` at two hops.

A native macOS Bluetooth node established both the BLE supervisor and a BLE
peer. A T096 announce carrying that same destination crossed the L2CAP data
plane and was independently observed by the host through `BluetoothPeer` at one
hop. The T096 BLE menu reported three live peers during the session. Parallel
dials to other nearby BLE advertisements sometimes received CoreBluetooth
`CBATTErrorDomain` resource errors; the established T096 peer and proven packet
path remained live.

## Persistence and GNSS

The LoRa profile was changed from the default `MediumFast` preset to
`LongFast`, saved, and recovered as `LongFast` after reset. The profile was then
reset to the board default and recovered as `MediumFast` after another reset.
The independently observed destination remained exactly
`42bd597b70358b5aecb050ce50688ce7` across those reboots, proving stable node
identity restoration alongside SoftDevice-safe profile persistence.

GNSS was enabled from the global menu. The UC6580 adapter displayed
`GPS SEARCH S00` / `Waiting for fix`, confirming the receiver enable path,
shared GNSS snapshot flow, and display integration. Because the satellite count
remained zero and no fix was obtained, this indoor session did not independently
prove valid NMEA sentence ingestion, a satellite position fix, or coordinate
accuracy.

## Remaining limits

This receipt does not qualify outdoor GNSS time-to-fix or coordinate accuracy,
RF output power or sensitivity against calibrated equipment, collision and
contention behavior, long-duration soak behavior, every regional radio profile,
or a signed release artifact. It records the exact developer UF2 above and the
physical paths exercised during this session.
