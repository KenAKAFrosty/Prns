# Heltec Vision Master E290-HF qualification

The E290-HF is a cataloged qualification target, not a shipping or publicly
signed-release target. This receipt separates two kinds of evidence that must
not be conflated:

- Andrew M qualified his implementation physically on two boards in PR #167 at
  commit `0b4a95d3be6286e73a249c48417f28d1036df1bc`.
- The canonical display architecture and E290 port were materially rewritten
  on local `trunk` in `c0aebcb9`, `7c473e79`, and `addd5683`. Those adapted
  binaries have extensive software evidence but have not been reflashed onto
  the two qualification boards.

Andrew's exact observations remain available in the
[source receipt at `0b4a95d3`](https://github.com/KenAKAFrosty/Prns/blob/0b4a95d3be6286e73a249c48417f28d1036df1bc/validation/qualifications/heltec-e290-qualification.md).
They establish useful hardware facts and failure discoveries; they do not by
themselves qualify a later rewritten artifact.

## Supported hardware contract

| Contract | Qualified target |
|---|---|
| Board | Heltec Vision Master E290-HF V0.3.1 |
| MCU and memory | ESP32-S3R8, 16 MiB DIO/40 MHz flash, 8 MiB octal PSRAM |
| Radio | Fitted HT-RA62-HF/SX1262 assembly; LF assembly unsupported |
| Display | DEPG0290BNS800F6 V2.1, SSD1680Z8, 296×128 monochrome |
| Input | Active-low GPIO21 key with the board's external pull-up |
| Board A | Operator-identified E290-HF, ESP USB serial `AC:A7:04:E1:3F:88` |
| Board B | Operator-identified E290-HF, ESP USB serial `AC:A7:04:E1:49:A4` |

The physical session detected ESP32-S3, 16 MiB flash, and 8 MiB mapped PSRAM on
both units. Exact PCB, panel, and fitted radio-module markings were not
transcribed during that session, so physical identity remains partial despite
the operator's board identification.

The target deliberately omits battery telemetry, GNSS, QuickLink expansion,
partial e-paper waveforms, OTA policy, and public release promotion. It exposes
no user Display Off action.

## Upstream powered findings

Powered work began on 2026-08-25. The complete source receipt above records the
developer versions, artifact hashes, device identifiers, trace hashes, and
remaining limits. Its durable findings are:

| Surface | Upstream result | Boundary retained by the adaptation |
|---|---|---|
| Doctor, sparse flash, verification, and boot | Passed on both boards | ESP32-S3, 16 MiB, DIO, 40 MHz, USB reset, watchdog reset, and the three sparse regions remain the catalog contract. |
| Display orientation and bounds | Passed after rejecting a mirrored candidate | The adapted driver preserves the qualified controller reflection, clockwise canonical transform, 20-pixel margins, and black-on-white packing. |
| Retained presentation timing | Partial | Three full waveforms measured 1.617–1.631 seconds; two telemetry intervals began 30.005 and 30.004 seconds after completion. Prolonged retention, system sleep, and a powered unchanged-frame quiet interval remain unproven. |
| Input during a waveform | Passed observationally | Short and long presses navigated; a press during refresh was preserved and applied afterward. Measured debounce bounds remain unrun. |
| Busy and controller recovery | Partial | Normal reset, RAM write, full refresh, and deep sleep stayed within bounds. Stuck-BUSY and disconnected-panel injection were not run. |
| HF LoRa | Passed | Isolated bidirectional announces, routed proofs, reset recovery, Links, and a Resource response crossed the two-board LoRa path. |
| ESP-NOW | Passed | Isolated bidirectional announces crossed ESP-NOW with other inter-board paths disabled. |
| Wi-Fi and configured TCP | Passed after a shared capacity correction | Both boards associated, obtained DHCP, connected, survived listener loss, and reconnected after power loss. |
| Bluetooth and USB Auto | Passed for exercised paths | Native BLE and USB sessions were established and recovered after reset; deliberate fault injection and long-duration concurrency remain limited. |
| Identity and configuration persistence | Passed | Distinct node and BLE identities, HSPCFG1, and radio profiles survived sparse flashes, resets, and physical power loss. |

The session rejected three defects before recording those results: mirrored
controller order, an SX1262 transmit path that could accept a stale DIO1 and a
non-`TxDone` IRQ, and an S3 socket budget that omitted the DNS socket. It also
removed an unrelated five-second post-waveform delay. These findings became
explicit requirements in the adapted implementation.

Stock Python RNS 1.4.2 completed the recorded announce, route, proof, Link, and
Resource paths. The corresponding RNS 1.5.0 session used a temporary generic
`medium_path_timeout` RPC compatibility patch that is not part of E290 support;
that run is transport evidence, not evidence that this branch implements the
missing RPC.

## Adapted software evidence

The rewritten port centralizes the canonical 64×128 frame, panel transform,
presentation policy, presentation knowledge, and transactional completion in
`personal-hopspot-core`. E290 supplies only its controller packing, physical
I/O, power rail, full-waveform policy, and 30-second telemetry floor. Input
accumulated during a slow waveform is drained as a bounded batch, and notice
lifetimes begin only after a successful physical presentation.

The local validation performed while adapting the port includes:

- 166 core tests and warnings-denied core Clippy;
- 45 ESP display/runtime host tests and warnings-denied host Clippy;
- 15 focused SX126x tests covering stale-high, wrong-IRQ, missing, and
  post-transmit-release behavior;
- exact release builds for E290, Heltec V4, Heltec V4-R8, and T-Beam Supreme;
- a headless XIAO ESP32-C6 release build without the `display` feature;
- Nordic T-Echo release checking and warnings-denied Clippy after the shared
  notice-timer move;
- 160 flasher and flash-manifest tests, 26 developer-flasher tests, and 43
  website tests in both default and E290 local-development profiles; and
- warnings-denied Clippy for the flasher, manifest, and both website profiles.

The real flasher build path produced an E290 sparse artifact from the adapted
tree with 1,887,152 bytes across bootloader, partition table, and application
parts. Its generated target record carried the expected ESP32-S3, 16 MiB,
DIO/40 MHz, USB-reset, watchdog-reset, HSPCFG1, and TCP provisioning contract.
That temporary artifact was inspected and removed; it was not flashed or
presented as release custody.

Cross-target E290 Clippy reaches the complete graph when the two pre-existing
shared S3 `too_many_arguments` lints are allowed. The exact target build and all
supported warnings-denied host gates pass without adding a board-local lint
exception.

## Remaining qualification

Before shipping promotion, the accepted commit and its exact artifact must be
flashed onto both recorded boards. That regression must at least repeat:

- physical marking transcription, doctor, sparse verification, boot, and PSRAM;
- front-facing orientation, margins, first render, unchanged-frame quiet time,
  retention through rail-off, and presentation after controller knowledge loss;
- short, long, and during-waveform input;
- isolated bidirectional LoRa and ESP-NOW;
- BLE, USB Auto, Wi-Fi, configured TCP, persistence, reset, and power loss; and
- stuck-BUSY or disconnected-panel recovery, or an explicit maintainer
  acceptance of that remaining limit.

Browser delivery, long-duration soak behavior, calibrated RF output and
sensitivity, every regional radio profile, and signed release custody also
remain outside this receipt. CI results cover software and target compilation;
they cannot substitute for the final adapted-artifact hardware regression.

Andrew M is credited for the original architecture direction, E290 bring-up,
two-board powered investigation, and the hardware defects that investigation
surfaced. The adapted commits preserve that contribution while replacing the
software ownership model.
