# Personal Hopspot

For T1000-E users switching back to Meshtastic or another application, see
[firmware recovery](embedded/nrf52840/RECOVERY.md). Compatible Hopspot firmware
supports recovery entry from the web flasher and a held-button startup path.

Personal Hopspot is one Reticulum-based node application across desktop, mobile,
and embedded platforms. It provides a status and control surface where the
platform has a display or interactive shell.

The `core` directory owns the platform-agnostic application state, canonical
64×128 monochrome face, display presentation policy, and renderer. Each entry
under `desktop/`, `mobile/`, and `embedded/` converts that canonical frame into
native pixels and binds the shared application and Reticulum node to its input,
eligible interfaces, controller I/O, power rails, and power readings.

Personal Hopspot is also the board-backed embedded reference application. A
screen is optional: display-bearing standalone workspaces explicitly enable
core's `display` feature, while headless boards run the node and expose their
supported remote controls without compiling the face or presentation surface.

The [headless host](headless/README.md) runs the shared node and pages on Linux
and other Rust hosts without a display. Its initial interface is explicit TCP;
the ThinkNode G4 procedure records physical MIPS bring-up and remaining radio
and installer qualification work.

## Public packages

The `sdk/hopspot` directory is the shared home of the Rust crate and npm package
named `hopspot`. Both are transparent, version-locked facades over the complete
`personal-rns` Rust and JavaScript APIs. They provide an alternate package name
without creating a second implementation, type system, protocol surface, or
release line.

## The built-in NomadNet page

Every hopspot serves small [micron](https://github.com/markqvist/NomadNet) pages about the project
at `/page/index.mu`, `/page/coming-from-rns.mu`, `/page/quickstart.mu`, and `/page/source.mu` on a
standard `nomadnetwork.node` destination, so any NomadNet-capable client who finds the node can
open them like any other node page. The index uses the same shared project face and navigation as
the daemon and browser node, including the complete Coming-from-RNS page. Large static pages remain
in flash and are served through bounded Resource windows instead of requiring one response-sized
RAM allocation. The self-contained quickstart remains directly available for existing links. The
source page links to the on-node archive when the build carries one and points compact builds to
the public source otherwise. Pressing Announce on a hopspot announces only this node destination;
the hopspot's private `lxmf.delivery` destination remains available without advertising itself as
an LXMF peer.

The platform-specific welcome and navigation fragments live in `core/src/node_pages/`; the common
masthead, project summary, license, quote, and credits live in `assets/nnpages/` and are shared with
the other node faces. Build-time composition emits `&'static` pages served straight from flash,
with no filesystem or duplicate prepacked copy. `core/src/node_pages.rs` owns the static request
endpoints, route sets, response-capacity accounting, and destination constants registered through
the node recipe on every face.

## Workspaces and toolchains

`core` is a member of the repository workspace. Every crate under `desktop/`, `mobile/`, and `embedded/` is its own standalone workspace with its own `Cargo.lock`. Each carries its own `rust-toolchain.toml`: e.g., `esp32` uses the Xtensa `esp` channel (espup) while most others build on stable.

## Building

Desktop, from `desktop/`:

    cargo desktop

On Windows the desktop build compiles SDL2 from the bundled source and links it
statically, which requires CMake and the Visual Studio Build Tools C++ workload
(MSVC). On macOS it links Homebrew's `sdl2` (`brew install sdl2`).

Native USB Auto discovers CDC, Prns WebUSB, already-enumerated Android Open
Accessory devices, and managed iOS devices. Set `PRNS_USB_AUTO_ANDROID_ACCESSORY`
to allow an ordinary Android device to be switched into accessory mode. An
explicit usbmux endpoint can be supplied through `PRNS_USB_AUTO_USBMUX_TARGET`,
or `PRNS_USB_AUTO_USBMUX_AUTO` can select `127.0.0.1:42700`. The older
`HOPSPOT_USBMUX_TARGET` and `HOPSPOT_USBMUX_AUTO` names remain lower-precedence
compatibility aliases.

ESP32 firmware, from `embedded/esp32/` with the board on USB:

    cargo heltec-e290-flash
    cargo heltec-v3-flash
    cargo heltec-v4-flash
    cargo heltec-v4-r8-flash
    cargo tbeam-supreme-flash
    cargo c6-flash

The Wireless Stick Lite V3 uses the catalog-owned serial reset policy. From the
repository root, build and flash the current working tree with:

    ./tools/prns run device.hopspot.flash-local -- heltec-wireless-stick-lite-v3 --monitor

Its [qualification receipt](../validation/qualifications/heltec-wireless-stick-lite-v3-qualification.md)
records the supported hardware contract, current software evidence, and the
remaining physical soak and sanity checks.

T-Echo firmware:

    ./tools/prns device techo flash

Heltec T096 and T114 developer firmware provide their factory TFT status and
control faces, Bluetooth Auto, and a 60-second display auto-off:

    ./tools/prns build hopspot t096
    ./tools/prns build hopspot t114

Heltec V3/V3.1 is available in the website flasher with BLE Auto, LoRa,
USB Auto through its CP2102 adapter, and the OLED face. Its no-PSRAM
firmware leaves Wi-Fi, TCP Client, and ESP-NOW disabled.

Seeed Wio Tracker L1 / L1 Pro firmware is available in the website flasher and drives the 128×64 OLED status
face (SSD1306 or SH1106, detected at boot), the user button and five-way
joystick, the L76K GNSS, and Bluetooth Auto on the factory S140 7.3.0 UF2
bootloader. Double-tap reset and copy the UF2 onto the bootloader drive:

    ./tools/prns build hopspot wio-tracker-l1

The website image is for the standard-radio OLED models, not the E-Ink or
1 W variants. Confirm the model label even when INFO_UF2.TXT reports the
shared `TRACKER L1` identity.

The L1 Pro 1W has its own developer build, which powers the radio's 1 W amplifier rail
and maps requested output power through the amplifier's gain curve:

    ./tools/prns build hopspot wio-tracker-l1-pro-1w

## Local developer web flasher

Build and serve the current working tree for one or more cataloged boards with:

    ./tools/prns run device.hopspot.dev-flasher.serve -- BOARD [BOARD ...] --port PORT

Supply multiple unique board slugs to test them together. Explicit selection
may include qualification targets; `--all` intentionally builds only the
shipping set:

    ./tools/prns run device.hopspot.dev-flasher.serve -- --all --port 8765

The [board catalog](../release/flash/boards.json) is the source of truth for
available slugs and lifecycle state. The command builds the selected firmware,
creates a private temporary candidate, signs its manifest and preview channel
with a newly generated ephemeral key, and serves the real flasher only on
`127.0.0.1`. Open the printed `/flash` URL in the browser under test.

Flashing erases and rewrites device flash and can destroy the installed
firmware and stored device state. Confirm the selected board and recovery path
before starting a flash. Press Ctrl-C to stop the server; the ephemeral secret
key and temporary candidate are removed as the process exits.

The browser trusts only the ephemeral public key compiled into that local
website build. This makes the assembled local candidate internally verifiable,
but it is not production signing, published release custody, or hardware and
browser qualification evidence. A successful local flash does not qualify a
signed release. Qualification receipts and their remaining limits live under
[`validation/qualifications/`](../validation/qualifications/).

## Embedded flash-layout upgrade

The [runtime entropy guide](../docs/runtime-entropy.md) documents board bring-up,
reseed sources, and the 0.4.0 full-erase remediation for ESP32-S3 identities
created on the affected 0.3.7 through 0.3.7-hotfix.4 first-boot path.

LoRa-capable firmware persists the selected radio profile in a dedicated two-page store. Reset records a durable choice to follow the firmware default, while an explicitly saved profile remains fixed across updates. Sparse firmware updates preserve the profile store; a full-chip erase clears it.

Every embedded Hopspot board target journals learned routes and retained self-ratchet history. Route writes are batched to conserve flash and battery, while critical ratchet state receives the shorter durability window. T114 uses the established T096 journal map, MeshTower V2 reserves its own six-page journal below the existing profile and identity pages, and the muzi Base Duo preserves those identity and learned-state addresses on the same S140 6.1.1 UF2 bootloader layout. Their first persistence-capable update starts with empty learned state; later reboots and sparse firmware updates restore it. A full-chip erase clears it.

The first firmware update carrying the board-sized flash layout moves learned-state persistence on the 16 MiB Heltec V4 and V4 R8 from the lower 8 MiB region to the physical flash tail. Node identity, Bluetooth identity, and Wi-Fi provisioning remain intact, but learned routes and retained self-ratchet history from older firmware are reset once and rebuild from network activity. The 8 MiB T-Beam Supreme journal remains in place. T-Echo keeps its journal timebase and arena starts while reserving the former final arena page, reducing the second arena from 20 pages to 19.

### SenseCAP Solar Node P1 / P1-Pro

`./tools/prns build hopspot sensecap-solar-node` builds the headless nRF52840 / Wio-SX1262 release UF2. The P1-Pro GNSS adapter controls the L76K receiver; LoRa starts unconfigured and uses the current Remote Control regional configuration rather than a board-wide fixed European channel. The node page announces after 15 seconds and every six hours.

The port adapts [PR #227](https://github.com/KenAKAFrosty/Prns/pull/227) to the shared memory-profile and Remote Control owners. Its firmware participates in the resource and architecture assurance matrix. The contributor's historical physical-board observations are recorded in that PR; automated evidence does not prove GNSS, RF, power, or physical-board behavior. The public catalog primarily identifies the contributor’s recorded Solar bootloader: `SENSECAP`, Board-ID `nRF52840-SeeedSenseCAPSolarP1-v1`, bootloader `0.9.2-OTAFIX2.2-BP1.3`, S140 7.3.0. It also accepts the exact Seeed recovery identity `nRF52840-SeeedXiao-v1` on `XIAO-BOOT`, verified against the [official Seeed recovery package](https://files.seeedstudio.com/wiki/SenseCAP/Meshtastic/xiao_nrf52840_ble_bootloader.zip) linked by the [Solar Node guide](https://wiki.seeedstudio.com/get_started_with_meshtastic_solar_node/). The application runs without the SoftDevice at `0x27000`; browser recovery requests set the UF2 flag directly before resetting. Confirm the Solar enclosure label because other XIAO products share that bootloader identity.

### XIAO ESP32-S3 with Wio-SX1262

The `hopspot-xiao-esp32s3-wio-sx1262` ESP workspace package builds the headless 8 MiB flash / 8 MiB Octal PSRAM board. Its flash catalog and memory profile use the normal ESP32-S3 resource and assurance gates. The SX1262 receive-enable pin and GPIO48 activity LED use the shared driver lifecycle callbacks, including cancellation cleanup.

The board port comes from [PR #239](https://github.com/KenAKAFrosty/Prns/pull/239). Current Remote Control owns regional radio configuration; the PR's older standalone flasher `configure` command and radio-profile storage format are not imported. That PR therefore still contains separate provisioning work beyond this board integration.

### RAK WisMesh 1W

`./tools/prns build hopspot rak10724` builds the RAK3401 / RAK13302 release UF2 adapted from [PR #247](https://github.com/KenAKAFrosty/Prns/pull/247). It uses the current S140 6.1.1 startup and regional radio controls, with the contributor's SKY66122 power mapping. Its memory profile participates in the canonical resource and architecture assurance matrix. The public catalog uses the shared `RAK4631` volume and `WisBlock-RAK4631-Board` identity with S140 6.1.1. The website requires kit confirmation because the WisBlock RAK4631 and WisMesh 1W recipes have different radio hardware and USB application identities.

The nRF runtime now loads the contributor's optional factory controller grant from the Remote Control identity vault. Retained permission snapshots replace initial grants before the node starts, including an empty table after revocation. This supports pre-provisioned identity pages; it does not provision a controller during an ordinary browser firmware installation. For ordinary browser installs, use the USB controller authorization flow below. The PR's unsigned developer-artifact flasher path is also separate from the signed public-release flow.

### USB replies and retained radio configuration

The nRF USB lane retains an announce and its control reply together, including
T-Echo and MeshPocket as well as the headless runtime. This incorporates the
fault identified and hardware-tested by Idan in [PR #261](https://github.com/KenAKAFrosty/Prns/pull/261).
Both USB endpoints advertise the packet size that the current USB Auto protocol
actually encodes; nRF lane buffers follow that same bound. The ESP runtimes
already reserve larger outbound bursts and enforce this minimum too.

The screenless RAK4631, WisMesh 1W, MeshTower V2, muzi Base Duo, T1000-E and
SenseCAP Solar Node now save remotely selected LoRa profiles and restore them
at startup, extending the RAK prototype from [issue #260](https://github.com/KenAKAFrosty/Prns/issues/260).
A successful change confirms durable storage. Failed writes restore the previous
radio profile; an uncertain commit also requires durably restoring that previous
profile before reporting a recovered failure. An unsuccessful rollback is
reported explicitly.

The memory profiles reserve two radio pages at `0xE0000..0xE2000` for the
RAK/MeshTower/Base Duo family and `0xE7000..0xE9000` for T1000-E/Solar.
Firmware bounds and flasher validation exclude these pages. Identity, journal,
factory and bootloader addresses stay fixed; the former unused single radio
page on the RAK/MeshTower family remains reserved. Display-equipped nRF and
ESP boards already retain profiles; all nRF and ESP remote profile changes now use the
same tested persistence and rollback implementation. This handles failed writes;
it does not add a timed confirmation protocol for changes made over the radio.

### Persistent node names and Wio OLED updates

All nRF Hopspot boards expose `SetNodeName` and `DescribeNodeName` through
authorized Remote Control, incorporating [PR #263](https://github.com/KenAKAFrosty/Prns/pull/263).
Names accept 1–64 UTF-8 bytes without control characters or surrounding
whitespace, including the existing longer factory names. The current journal
retains the name across reboot and compaction without moving identity or storage
regions. A successful command confirms persistence and updates both the LXMF
delivery and node-page announcements. Retrying after an announcement failure
reapplies both announcements even when the name is already durable. ESP32
firmware does not yet advertise these two commands.

The Wio Tracker L1 and Pro 1W incorporate the OLED update from
[PR #259](https://github.com/KenAKAFrosty/Prns/pull/259): only changed panel pages
are transferred, with a full redraw after an uncertain or partial I2C write.
The integration keeps the page cache statically allocated. Host tests inject
partial transfers at every page and check retry recovery; these do not replace
physical display or bus testing.

### First controller on an nRF board

Install Hopspot, let it start, then use **Connect your controller** on that
board's flasher page in desktop Chrome or Edge. Paste the controller's full
128-character public key and choose **Authorize controller over USB**. The USB
picker checks the selected board's Hopspot application identity. Setup grants
that controller Administrator access, including radio configuration and grant
management. It reports success only after the existing authorization journal
has durably saved the grant. Save the returned board public key in the controller
to pin the target identity. Neither private key leaves its owning device.

A persistent development controller can print its public key using the
[headless controller example](headless/docs/remote-control.md#provision-and-operate).
Retain the controller's private state directory; generating another identity
requires authorizing its new public key. USB Auto carries the subsequent
identity-authenticated Remote Control traffic. A radio starts unconfigured until
its region/profile is explicitly selected by an authorized controller.

USB enrollment is an explicit local administrative operation, available whenever
a trusted host has access to the board's USB control interface. It does not open
radio pairing or accept unauthenticated network requests. It can also authorize
a replacement controller without erasing firmware, identities, or other grants.
A disconnected or timed-out browser must not infer success; reconnect and retry
with the same public key. Repeating the operation is idempotent. Revoked grants
stay revoked across ordinary restarts and firmware updates; only an explicit
new authorization can restore access.

This requires the 0.3.8 USB enrollment implementation. Automated tests exercise
its wire contract, browser failure handling, and the journal's power-loss and
revocation behavior. Physical USB behavior remains outside simulator/emulator
coverage.


### Base Duo dual-band LoRa

The Base Duo has one LR1121 radio and one active LoRa interface. Its firmware
opts into the nondefault `lora-2g4` feature; SubG-only builds omit that band.
Fresh devices remain unconfigured until a profile is explicitly saved.

The 2.4 GHz balanced preset is 2445 MHz, SF7, BW812, CR4/5, 10 dBm, with an
18-symbol preamble. Manual profiles use `G,frequency_hz,sf,bw,cr,power_dbm,preamble`;
for example `G,2445000000,7,8,5,10,18`. Bandwidth codes 3, 4 and 8 select
203, 406 and 812 kHz. The board accepts power up to 11 dBm. Existing SubG
`L,...` profile text remains compatible.

`InspectRadio` reports supported bands, operating state, and the last confirmed
saved configuration separately. `ConfigureRadio` saves a profile or clears it.
These operations require an authorized controller grant containing those request
kinds; grants created before these operations existed may need replacement.
The headless Base Duo exposes them through Remote Control. Display-capable
integrations can use the shared dual-band editor and supply their board's power
ceiling explicitly.

An already-authorized controller can use the example through a TCP Reticulum
bridge that reaches the Base Duo's USB or Bluetooth interface:

```sh
cargo run --locked -p personal-rns --example radio_control \
  --features tokio-host,tcp,lora-2g4 -- \
  HOST:PORT TARGET_DESTINATION_HEX CONTROLLER_KEY_FILE LOCAL_TARGET_KEY_FILE balanced
```

Use `inspect`, `clear`, or explicit profile text in place of `balanced`. Both key
files contain separate raw 64-byte private identities. The controller key must
match the grant on the target. The local target key belongs to the example's own
node. Press the Base Duo announce button when prompted. The example checks the
target's advertised capabilities before configuring its single LoRa interface.

Changes finish the current logical transmission, pause the radio, stage the
hardware, save the journal, and await runtime publication before traffic resumes.
Changing channel identity discards old queued transmissions and partial receive
state; changing only power or preamble preserves queued transmissions. Failed
changes restore the prior configuration. Indeterminate restoration leaves the
radio unavailable and reports `RecoveryRequired`; reboot reloads the journal.
Saving a disabled radio changes its desired configuration without activating RF.
Requester disconnection does not cancel an admitted save.

Base Duo's journal uses the existing pages at `0xE0000` and `0xE1000`,
with the firmware ceiling at `0xE0000`. Identity, learned-state, Bluetooth,
and bootloader addresses stay fixed. A build
without 2.4 GHz support rejects a committed 2.4 GHz record rather than reviving
an older SubG configuration.

Hardware references: the manufacturer's [module datasheet](https://cdn.shopify.com/s/files/1/0657/6973/4201/files/nRFLR1121_Wireless_Transceiver_Module_Datasheet_V1.1.pdf?v=1767888139)
shows a separate filtered 2.4 GHz path, and the [Base Duo schematic](https://cdn.shopify.com/s/files/1/0657/6973/4201/files/Base_Duo_Schematic_Rev01.pdf?v=1766439439)
connects it to its own antenna connector. The [LR1121 manual](https://www.mouser.com/pdfDocs/UserManual_LR1121_v1_1.pdf)
owns the HF PA and modulation commands. New-band timing follows [Semtech
SWDR001 at a333238](https://github.com/Lora-net/SWDR001/blob/a333238acfa0a9dee9ce2824ce52e89b98f3d24b/src/lr11xx_radio.c),
including whole coded symbol blocks and nominal bandwidth values.
