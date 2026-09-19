# Embedded Prns

Embedded Prns is the same protocol engine and node-recipe API used by native applications, hosted by Embassy with fixed storage and hardware-specific interfaces. Personal Hopspot is the repository's board-backed reference application; it is a useful place to learn the boundary between reusable Prns code and device bring-up.

## Start with a build, not a flash

The XIAO ESP32-C6 is the smallest complete reference path. It is headless and uses USB, ESP-NOW, and Bluetooth, so this example punctuates the fact that an embedded node does not need a display or Wi-Fi LAN.

The Seeed XIAO ESP32S3 with its Wio-SX1262 B2B baseboard is the compact headless LoRa path. It runs the complete ESP32-S3 Hopspot interface set and starts with the EU868 profile; persisted radio settings take precedence after first boot. Build it without flashing from the ESP32 workspace:

```console
cd personal-hopspot/embedded/esp32
cargo xiao-s3-wio --locked
```

Install the repository's ESP Rust toolchain once:

```console
cargo install espup --version 0.17.1 --locked
espup install --targets esp32s3
rustc +esp --version
```

These commands prepare an ordinary developer workstation. Release builds use the repository's isolated, reproducible toolchain task. Build the real firmware without touching a connected device:

```console
cd personal-hopspot/embedded/esp32
cargo c6 --locked
```

That shortcut expands to the `hopspot-xiao-esp32-c6` release build for `riscv32imac-unknown-none-elf`, including `-Zbuild-std=core,alloc`. The workspace has its own lockfile and selects the `esp` toolchain through `personal-hopspot/embedded/esp32/rust-toolchain.toml`.

## Follow the recipe through the board

The important files form a short path:

1. `personal-hopspot/embedded/esp32/boards/xiao-esp32-c6/src/main.rs` is the board binary. It only hands the Embassy spawner to the shared C6 application.
2. `personal-hopspot/embedded/esp32/src/c6/board.rs` owns clocks, peripherals, USB, radio handles, the hardware MAC, and the hardware-backed timebase.
3. `personal-hopspot/embedded/esp32/src/c6/firmware.rs` creates fresh or persisted identities, claims interface lanes, constructs `PrnsNodeRecipe`, and runs the node manifold.
4. `personal-hopspot/embedded/esp32/src/storage.rs` is the explicit fixed-memory budget. It is part of the application design, not hidden allocator magic.
5. `personal-hopspot/core/src/node_pages.rs` is the reusable static-route example that serves the built-in NomadNet index and quickstart.

The center of the firmware is a `PrnsNodeRecipe`: transport identity, application destinations, storage, routes, interfaces, application state, and an event callback. Hardware code supplies those same obligations under `no_std`; it does not switch to a separate networking API.

## Understand the bounded choices

Every embedded board deliberately uses a bounded storage profile. Firmware carries the NomadNet index and quickstart, but it does not embed the multi-megabyte release source archive or register its file routes. The exact source snapshot remains available from the hosted release surfaces.

When adapting the recipe to a new board, decide these explicitly:

- where identity material persists and where boot entropy comes from;
- which interface drivers exist and how many simultaneous lanes or peers fit;
- the fixed capacities for destinations, links, resources, receipts, and packet history;
- which application routes and destinations the device owns;
- how tasks are supervised and how failure is reported without a desktop process around them.

Start from the nearest shipped board instead of copying the native two-node example and guessing at these obligations.

The [runtime entropy contract](runtime-entropy.md) defines the required
initialization order, continuous CSPRNG, reseeding behavior, and platform
transition rules. New boards must construct that branded service before any
identity/bootstrap work; consumers cannot substitute an arbitrary callback.

## Flash only when you mean to

Building does not require a board. Flashing does, and writes the selected device. Install the pinned flasher, connect a XIAO ESP32-C6, verify the target, then opt into the flash command:

```console
cargo install espflash --version 4.5.0 --locked
cargo run --locked -p hopspot-flash -- doctor xiao-esp32-c6
cd personal-hopspot/embedded/esp32
cargo c6-flash --locked
```

The doctor step is read-only. The final command flashes and opens a serial monitor. For signed release firmware, board discovery, and the supported operator flow, use the flasher described in [Personal Hopspot](../personal-hopspot/README.md).

### Provision the headless XIAO LoRa profile

The XIAO ESP32-S3 + Wio-SX1262 has no display. Provision its persistent LoRa
profile as part of the sparse flash operation:

```console
cargo prns-esp32-flasher flash xiao-esp32s3-wio-sx1262 \
  --local-build \
  --region eu869 \
  --frequency-hz 869527000 \
  --spreading-factor 9 \
  --tx-power-dbm 14 \
  --bandwidth-khz 250 \
  --coding-rate 4/5 \
  --announce-mm 15 \
  --node-name "Nodo tetto Roma" \
  --yes
```

`--region` selects the regulatory limits and is required whenever any radio
option is present. The other values override the regional preset. Omitting the
overrides uses the region's default frequency and maximum permitted power plus
the firmware's default modulation and preamble. Invalid combinations are
rejected before a serial port is opened or any flash is written.

The flasher clears both reserved radio-profile pages, writes the new profile to
the initial slot, and verifies it with the rest of the sparse image. Later
firmware updates preserve the profile unless they explicitly provision another
one.

After firmware is installed, update only the two persistent radio-profile pages:

```console
cargo prns-esp32-flasher configure xiao-esp32s3-wio-sx1262 \
  --region eu869 \
  --frequency-hz 869527000 \
  --spreading-factor 9 \
  --tx-power-dbm 14 \
  --bandwidth-khz 250 \
  --coding-rate 4/5 \
  --announce-mm 15 \
  --node-name "Nodo tetto Roma"
```

The command verifies the selected ESP32-S3, erases and writes only those two
pages, verifies the write on the device, and resets it. Firmware, Wi-Fi
provisioning, identities, and retained node state remain untouched.

`--announce-mm 15` enables a headless-node announce every 15 minutes. The
setting is stored beside the persistent LoRa profile, survives reboot and later
firmware flashes, and sends the first announce shortly after startup before
repeating it on all active interfaces.

## Verify embedded changes

Use the cheapest relevant rung first:

```console
cargo build --locked -p personal-rns --no-default-features
cargo build --locked -p personal-rns --no-default-features \
  --target riscv32imac-unknown-none-elf
bash validation/platforms/no-std-esp-build.sh
```

(The last rung is a bash script; on Windows run it from Git Bash, which
installs with Git for Windows.)

The Linux `embedded-builds` validation suite adds the Embassy interface
cross-builds, both S140 6.1.1 and 7.3.0 T-Echo firmware layouts, the
display-equipped Heltec T096 and T114 with Bluetooth Auto and display auto-off,
and the headless T1000-E and MeshTower V2 developer UF2s. Every embedded
Hopspot board target restores learned routes and retained self-ratchet history
from its board-owned flash journal:

```console
python3 validation/run.py run --suite embedded-builds --platform linux
```

Release qualification builds the flashable boards through the release-custody path. See the [testing guide](testing.md) before choosing a broader lane.
