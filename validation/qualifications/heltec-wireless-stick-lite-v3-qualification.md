# Heltec Wireless Stick Lite V3 qualification

The Wireless Stick Lite V3 is a cataloged qualification target, not a shipping
or publicly signed-release target. Its firmware and release integration are
complete at the software boundary. Promotion remains blocked on physical
qualification of an exact accepted artifact.

## Supported hardware contract

| Contract | Qualification target |
|---|---|
| Board | Heltec Wireless Stick Lite V3 |
| MCU and memory | ESP32-S3FN8, 8 MiB DIO/40 MHz flash, no PSRAM |
| Radio | Fitted SX1262 assembly with a 1.8 V TCXO, DCDC, and DIO2 RF switching |
| Interfaces | Bluetooth Auto, LoRa, and USB Auto |
| Delivery | ESP serial sparse flash with default-reset before flashing and hard-reset afterward |
| Configuration | Persistent regional LoRa profile; firmware default is US915 until changed through the supported profile path |

ESP32-S3 and 8 MiB flash detection cannot distinguish this board from every
other cataloged board with the same chip and capacity. Doctor output therefore
requires the tester to confirm the physical product and fitted radio markings
before selecting the Wireless Stick Lite image.

## Bring-up observation

The initial USB session detected an ESP32-S3 with 8 MiB flash on
`/dev/ttyUSB0`, completed a sparse flash with verification, booted the
Wireless Stick Lite firmware, and emitted the expected USB heartbeat. This
establishes the serial reset policy, flash geometry, boot path, and one-way USB
transmit smoke on the attached unit.

That observation used a local working-tree artifact rather than an immutable
signed candidate. It does not establish Bluetooth data transfer, bidirectional
USB Auto, LoRa RF behavior, profile persistence, fault recovery, coexistence,
or long-duration stability and is not sufficient for shipping promotion.

## Software and release evidence

The non-hardware boundary is covered by reproducible repository gates:

- the ESP firmware gate compiles this target independently with the pinned
  Xtensa toolchain, preventing feature unification with another board target;
- dependency policy covers its exact target graph for advisories, licenses,
  sources, and bans;
- the reviewed unsafe snapshot includes its target graph, while the board
  package itself contains zero unsafe tokens;
- manifest, native flasher, website, sparse-size, acceptance, roster, and
  candidate-custody tests derive the shipping set from the board catalog;
- release candidates carry and byte-check the catalog projection helper they
  use, so later source-tree policy changes cannot rewrite historical candidate
  behavior; and
- a dry promotion changing only this board's catalog availability to
  `shipping` passes the manifest, website, roster, acceptance, and release
  policy suites.

The generated physical acceptance rows require every interface declared by
the signed manifest. For this board, both CLI and browser assignments therefore
require Bluetooth Auto, LoRa, and USB Auto evidence in addition to flash,
verification, recovery, boot, and same-chip board confirmation.

## Remaining physical qualification

Qualify one exact accepted commit and record the artifact SHA-256, build
identity, board revision, fitted radio/SKU markings, USB identity, and peer
artifacts. On that exact image:

- repeat doctor, sparse flash, whole-part verification, boot, manual board
  confirmation, an interrupted-flash recovery, reset, and cold power-cycle;
- prove bidirectional USB Auto traffic, disconnect and reconnect, reset
  recovery, and sustained framing without corruption;
- prove Bluetooth discovery, connection, bidirectional traffic, disconnect and
  reconnect, and reset recovery;
- prove isolated bidirectional LoRa traffic against an independently observed
  peer on the region appropriate to the fitted hardware;
- save a non-default compatible radio profile and prove it across reset, cold
  power loss, and a subsequent sparse firmware flash before restoring the
  intended release profile;
- exercise Bluetooth, LoRa, and USB concurrently and confirm that activity on
  one interface does not starve or corrupt the others; and
- run an eight-hour mixed-interface powered soak with periodic bidirectional
  traffic, no panic or watchdog reboot, no unexpected identity change, and a
  final successful exchange on all three interfaces.

Calibrated RF output and sensitivity, every regional profile, battery
telemetry, and environmental testing are outside this target's initial
shipping receipt unless the release owner explicitly adds them to the supported
contract.

After that receipt passes, promotion is the controlled catalog availability
change from `qualification` to `shipping`. The ordinary signed-candidate
pipeline then derives the production manifest, firmware set, website target,
tester roster, sparse-size report, and exact physical CLI/browser acceptance
rows without another board-specific code or matrix change.
