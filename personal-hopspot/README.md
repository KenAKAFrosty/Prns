# Personal Hopspot

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

### Heltec V3 Remote Control enrollment (qualification)

The V3 uses 8 MiB flash without PSRAM and the explicit
`embedded/esp32/partitions-hopspot-8mb-v3.csv` layout. Its RC vault is one
4096-byte page at `0x67d000`. The shared ESP identity loader restores the target
identity and factory Operator grant from that page; without a valid factory
grant it starts with no factory authorization and retains the manual pairing path.

From the repository root, with the ESP toolchain environment loaded:

```console
rtk cargo run --locked -p hopspot-flash -- build heltec-v3 --out-root /tmp/v3-artifacts
```

After backing up device state and explicitly confirming the hardware write:

```console
rtk cargo run --locked -p hopspot-flash -- flash heltec-v3 --local-build --yes --port /dev/cu.usbserial-0001 --rc-vault /tmp/prns-rc-vault-heltec-v3.bin --rc-vault-offset 0x67d000
```

Use a vault minted by the managing Controller for its Operator; it contains
private key material and must not be published. Enrollment replaces the RC
identity, so old Controller target access must not be assumed to survive. The
flasher checks the vault page against the canonical memory profile, rejects
overlapping firmware parts, and erases only the vault sector before writing it.
It does not request a full-chip erase. Without `--rc-vault`, the RC vault is preserved.

This port reuses the enrollment implementation from `tonycowan/Prns`
(`feat/controller-multi-arch`, `2752d943963d`) and the ESP vault write changes
from `7ab8fe7f3`, under the repository's MIT/Apache-2.0 licenses. On 2026-10-03,
Controller-driven enrollment was tested on V3: immediate authenticated BLE
inventory and the management whitelist worked without manual pairing, including
after a power cycle and reopening Controller. The tested local Controller and
flasher needed a temporary launcher to pass the serial port explicitly and use
`--local-build` instead of the unsupported `--developer-artifacts` option. This
does not qualify the unadapted Controller/flasher integration or a published image.

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

The Heltec WiFi LoRa 32 V3 integration build uses its 8 MiB/no-PSRAM profile:

    cargo heltec-v3

It currently compiles the RC service alongside the USB-C connector's CP2102
UART0 path, BLE and SX1262 LoRa. Wi-Fi, ESP-NOW and TCP are disabled in this
SRAM-only runtime.
The restored OLED uses the shared Hopspot face: USB/LoRa/BLE cards and counters,
local menus, Announce, interface power, SubG configuration, Bluetooth discovery
groups, display blanking/auto-off and interface sleep/wake. Local controls and
RC commands share the same interface and radio-configuration owner. Static face
storage and separate firmware/UI-RC executor tasks preserve the no-PSRAM startup
stack fix; compact glyphs borrow slices of the original text.

BOOT short presses move the selection; hold for at least half a second and
release to select. The first press after display auto-off only wakes it.
To pair manually, start PRNS Controller with BLE or USB enabled. With the global
row selected, hold BOOT to open the menu, select `Pair remote`, then hold and
release to open the two-minute invitation. Enter the eight-character OLED
invitation in Managed Nodes and compare the six-digit confirmation on both
screens. The OLED discloses full controller access and initially selects
rejection. If the codes match, approve in Controller, briefly press BOOT to
select approval on the OLED, then hold and release to confirm. A long press
closes the invitation or completed pairing screen. Do not share either code.
Missing/failed display or pairing-persistence failure disables local approval;
a press started before the confirmation was displayed cannot approve it.

The common RC service also accepts the factory grant described in the enrollment
section above. Manual Announce requests both the node page and stable RC target;
restored pairing grants trigger the shared bounded target announcement sequence
once an interface is online.

The earlier `0.3.7-v3-ui-candidate3+fb7486f` hardware tests confirmed readable
OLED counters, local menu/Announce, menu pairing, Managed Nodes membership and
authenticated inventory after rebooting V3 and reopening Controller. The UI
source was recovered in `253fce8ff`; that combined build was flashed during the
enrollment test above. The earlier OLED/menu observations were not repeated on
the combined build. Battery calibration, sustained-load stability and RF
end-to-end remain unverified. The tested Controller still displays Offline
despite successful BLE RC responses, and DescribeNetworkTransport is unsupported
by the tested firmware (`UnknownRequestKind 32`). USB Waiting is not proof of an
active USB transport; the recorded management requests used BLE.
See [the integration evidence](../HELTEC_V3_REMOTE_CONTROL_SPEC.md) for the
revision-specific checks and outstanding gates.

T-Echo firmware:

    ./tools/prns device techo flash

Heltec T096 and T114 developer firmware provide their factory TFT status and
control faces, Bluetooth Auto, and a 60-second display auto-off:

    ./tools/prns build hopspot t096
    ./tools/prns build hopspot t114

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
reseed sources, and the 0.3.7-hotfix.5 full-erase remediation for ESP32-S3
identities created on the affected 0.3.7 through 0.3.7-hotfix.4 first-boot path.

LoRa-capable firmware persists the selected radio profile in a dedicated two-page store. Reset records a durable choice to follow the firmware default, while an explicitly saved profile remains fixed across updates. Sparse firmware updates preserve the profile store; a full-chip erase clears it.

Every embedded Hopspot board target journals learned routes and retained self-ratchet history. Route writes are batched to conserve flash and battery, while critical ratchet state receives the shorter durability window. T114 uses the established T096 journal map, MeshTower V2 reserves its own six-page journal below the existing profile and identity pages, and the muzi Base Duo shares that MeshTower V2 map because both sit on the same S140 6.1.1 UF2 bootloader layout. Their first persistence-capable update starts with empty learned state; later reboots and sparse firmware updates restore it. A full-chip erase clears it.

The first firmware update carrying the board-sized flash layout moves learned-state persistence on the 16 MiB Heltec V4 and V4 R8 from the lower 8 MiB region to the physical flash tail. Node identity, Bluetooth identity, and Wi-Fi provisioning remain intact, but learned routes and retained self-ratchet history from older firmware are reset once and rebuild from network activity. The 8 MiB T-Beam Supreme journal remains in place. T-Echo keeps its journal timebase and arena starts while reserving the former final arena page, reducing the second arena from 20 pages to 19.
