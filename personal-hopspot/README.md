# Personal Hopspot

Personal Hopspot is one Reticulum-based application rendered across many platforms. It features a status screen and control surface for a Personal Reticulum (Prns) node. It runs on desktop, mobile, and embedded.

The `core` directory holds the platform-agnostic screen renderer. Each entry under `desktop/`, `mobile/`, and `embedded/` binds that renderer & control surface (along with Reticulum) to one platform: its display, user input, eligible interfaces, and power source readings. Adding a platform means adding one directory that fills in those platform-specific pieces. 

> NOTE: Having a screen is not necessary. Embedded devices without screens can still run this Hopspot application and forego the renderer and control surface. This is common, expected, normal behavior, as most embedded devices are used as relays and/or remote-controlled from another Reticulum-based application on a standard host machine. Hopspot is *the* canonical way to run a Prns node on embedded devices of all kinds.

## The built-in NomadNet page

Every hopspot serves a small [micron](https://github.com/markqvist/NomadNet) landing page about
the project at `/page/index.mu` on a standard `nomadnetwork.node` destination, so any NomadNet
or MeshChat user who finds the node can open it like any other node page. Pressing Announce on
a hopspot announces this node destination alongside the usual `lxmf.delivery` one.

The page lives in `core/src/node_pages/` (head and tail spliced at build time around a line naming what serves it) and is served as `&'static` bytes straight
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
    cargo tbeam-supreme-flash
    cargo c6-flash

T-Echo firmware:

    ./tools/prns device techo flash
