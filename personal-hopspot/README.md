# Personal Hopspot

Personal Hopspot is one application rendered across many platforms: a status screen and control
surface for a Personal Reticulum (Prns) node. It runs as a desktop window, as ESP32 and nRF52840
firmware, and inside Android and iOS apps.

It serves four roles at once: a shipping app (most of all on embedded boards with a display), a live
diagnostics view while Prns is built, the proving ground for the Prns consumer API, and a worked
example of integrating Prns into an application.

## Layout

    core/                  shared, portable renderer (crate personal-hopspot-core)
    platform_impls/
        desktop/           Linux/macOS/Windows debug window
        esp32/             ESP32-S3 (Heltec V4, T-Beam Supreme) and ESP32-C6 (XIAO) firmware
        t-echo/            LilyGO T-Echo (nRF52840 + e-ink) firmware
        android/           Kotlin shell + Rust JNI bridge
        ios/               Swift shell + Rust C-ABI bridge

`core` holds the platform-agnostic screen renderer that every face draws. Each entry under
`platform_impls/` binds that renderer to one platform: its display, input, transports, and power
source. Adding a platform means adding one directory that fills in those platform-specific pieces.

Inside `esp32/`, the firmware is layered by chip family, chip, and board: shared code at the crate
root (`ble.rs`, `storage.rs`), the per-chip core in `s3/` and `c6.rs`, and the thin per-board
implementations in `s3/boards/`. A board provides only what differs (its pin map, display, and
battery source) through the `Esp32S3Board` seam.

## Workspaces and toolchains

`core` is a member of the repository workspace. Every entry under `platform_impls/` is its own
standalone workspace with its own `Cargo.lock`, so the repository workspace's feature unification and
lint gates never pull embedded or platform dependencies into the engine build. Each carries its own
`rust-toolchain.toml`: `esp32` uses the Xtensa `esp` channel (espup); the others build on stable.

## Building

Desktop, from `platform_impls/desktop/`:

    cargo desktop

ESP32 firmware, from `platform_impls/esp32/` with the board on USB:

    cargo heltec-v4-flash
    cargo tbeam-supreme-flash
    cargo c6-flash

T-Echo firmware:

    scripts/techo-flash.sh

The web-flasher artifacts are produced by `cargo run -p hopspot-flash`. Android builds through Gradle
under `platform_impls/android/`; iOS through the Xcode project under `platform_impls/ios/`.
