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
at `/page/index.mu` and `/page/quickstart.mu` on a standard `nomadnetwork.node` destination, so any
NomadNet-capable client who finds the node can open them like any other node page. The index
links to the self-contained quickstart, which covers a daemon, a Rust consumer, an actual embedded
firmware build, tests, and benchmarks without requiring the public website. Pressing Announce on a
hopspot announces this node destination alongside the usual `lxmf.delivery` one.

The pages live in `core/src/node_pages/` (the index head and tail are spliced at build time around
a line naming what serves it) and are served as `&'static` bytes straight
from flash, with no filesystem or duplicate prepacked copy. `core/src/node_pages.rs` is the
reference example for static serving over Reticulum's request/response mechanism: a
`RequestRoute` that answers with `respond_static_bytes`, a named `RouteSet`, and the destination
constants, all registered through the node recipe on every face.

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
