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

LoRa-capable firmware persists the selected radio profile in a dedicated two-page store. Reset records a durable choice to follow the firmware default, while an explicitly saved profile remains fixed across updates. Sparse firmware updates preserve the profile store; a full-chip erase clears it.

Every embedded Hopspot board target journals learned routes and retained self-ratchet history. Route writes are batched to conserve flash and battery, while critical ratchet state receives the shorter durability window. T114 uses the established T096 journal map, and MeshTower V2 reserves its own six-page journal below the existing profile and identity pages. Their first persistence-capable update starts with empty learned state; later reboots and sparse firmware updates restore it. A full-chip erase clears it.

The first firmware update carrying the board-sized flash layout moves learned-state persistence on the 16 MiB Heltec V4 and V4 R8 from the lower 8 MiB region to the physical flash tail. Node identity, Bluetooth identity, and Wi-Fi provisioning remain intact, but learned routes and retained self-ratchet history from older firmware are reset once and rebuild from network activity. The 8 MiB T-Beam Supreme journal remains in place. T-Echo keeps its journal timebase and arena starts while reserving the former final arena page, reducing the second arena from 20 pages to 19.
