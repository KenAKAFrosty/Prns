# Personal Hopspot

Personal Hopspot is one Reticulum-based node application across desktop, mobile,
and embedded platforms. It provides a status and control surface where the
platform has a display or interactive shell.

The `core` directory holds the platform-agnostic screen renderer. Each entry
under `desktop/`, `mobile/`, and `embedded/` binds the shared application and
Reticulum node to the platform's display, input, eligible interfaces, and power
readings.

Personal Hopspot is also the board-backed embedded reference application. A
screen is optional: headless boards run the node and expose their supported
remote controls without compiling a display surface.

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

ESP32 firmware, from `embedded/esp32/` with the board on USB:

    cargo heltec-v4-flash
    cargo heltec-v4-r8-flash
    cargo tbeam-supreme-flash
    cargo c6-flash

T-Echo firmware:

    ./tools/prns device techo flash

## Embedded flash-layout upgrade

LoRa-capable firmware persists the selected radio profile in a dedicated two-page store. Reset records a durable choice to follow the firmware default, while an explicitly saved profile remains fixed across updates. Sparse firmware updates preserve the profile store; a full-chip erase clears it.

The first firmware update carrying the board-sized flash layout moves learned-state persistence on the 16 MiB Heltec V4 and V4 R8 from the lower 8 MiB region to the physical flash tail. Node identity, Bluetooth identity, and Wi-Fi provisioning remain intact, but learned routes and retained self-ratchet history from older firmware are reset once and rebuild from network activity. The 8 MiB T-Beam Supreme journal remains in place. T-Echo keeps its journal timebase and arena starts while reserving the former final arena page, reducing the second arena from 20 pages to 19.
