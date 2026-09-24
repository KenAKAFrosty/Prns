# ThinkNode G4: headless bring-up and future installer procedure

Status: physical TCP bring-up and a native HaLoW page transfer verified on one
G4 and one Heltec, 2026-09-29. The native test retained their vendor AP/station
connection; a subsequent [three-node native mesh qualification](qualification/halow-three-node-2026-09-29.md)
also passed basic transfers and application rejoin. The optional WebSocket build
passed [real browser access through HaLoW](qualification/websocket-halow-2026-09-29.md). Sustained load and forced
multi-radio-hop behavior remain unqualified. Separate radio
experiments measured group delivery and unicast throughput. The
[application slot manager](../../appliance/README.md) supplies persistent procd
startup, separate private state, signed compressed slots and bounded trial/rollback.
Firmware replacement and physical power-loss restore remain unqualified. This
is a development procedure, not a shipping board entry.

## Observed platform

The inspected unit runs the vendor OpenWrt image on an MT7628AN, MIPS24KEc,
580 MHz, with about 117 MiB usable RAM and 32 MiB SPI flash. Linux is 5.15.150;
the installed userspace uses musl 1.2.4 and the mipsel_24kc soft-float ABI.
The MM6108 HaLow radio uses the vendor Morse SDIO driver/firmware. Preserve that
working driver, firmware, regulatory configuration, and board calibration while
qualifying the application layer.

The remaining writable overlay was only 5,368 KiB before this deployment. The
initial 3.2 MiB TCP executable fits in RAM comfortably; the later HaLoW plus
WebSocket candidate is 3,710,604 bytes. Simply copying it into flash
would leave limited headroom for state, logs, updates, and rollback. A persistent
installation needs a space budget, not just a successful copy.

## Verified cross-build

The physical run used Zig 0.15.2 and Rust
`1.98.0-nightly (6bdf43094 2026-06-01)` with `rust-src`. The target has no prebuilt
Rust standard library in this toolchain; `build-std` is required. The Zig
aarch64-macos archive used during development had SHA-256
`3cc2bab367e185cdfb27501c4b30b1b0653c28d9f73df8dc91488e66ece5fa6b`.
That digest identifies that exact host archive, not every Zig distribution.

Install `nightly-2026-06-02` with `rust-src` and verify Zig separately. From the repository root,
use the repository task to build an application bundle:

```sh
./tools/prns build hopspot g4 -- \
  --zig "$ZIG" --output target/hopspot-g4/candidate
```

The task checks the compiler commit and Zig version, uses a locked Cargo build
with `build-std=std,panic_abort`, and rejects the wrong ELF architecture or a
dynamic loader. `--toolchain` defaults to the dated `nightly-2026-06-02` toolchain; its compiler
must match the pinned commit. The output directory must be new. The bundle includes
the executable, this procedure, licenses, third-party notices, `build.json`, and
`SHA256SUMS`.
It is unsigned development output; checksums do not establish release authenticity
or independent reproducibility. No upload or device modification occurs.

`--with-auto-wifi` includes optional Auto-WiFi and native DNS-SD. Runtime
participation still requires an explicit `--auto-wifi-device` selection; consult
[the networking guide](networking.md) before selecting a vendor bridge.
Static browser assets remain a separate optional bundle.

The linker wrapper selects `mipsel-linux-musleabi`, `mips32r2`, and `-msoft-float`.
The binary is statically linked; this does not qualify dynamic compatibility
with the device's musl. Portable 64-bit atomics preserve the runtime's values
and orderings on this CPU. Their fallback is not assumed to be lock-free.

Record the source revision, full compiler versions, lockfile, flags, executable
length, and SHA-256 with every candidate. Verify the ELF architecture before
copying it. The observed linker warning about an absent prebuilt Rust target
library directory was nonfatal; built std and Zig supplied the linked libraries.

## Temporary deployment procedure

1. Identify the actual board and running firmware. Verify CPU/ABI, free RAM,
   overlay space, current listeners, and a management path that survives the test.
   Discover its address rather than assuming every device uses 192.168.12.1.
2. Authenticate SSH and record/verify its host key. This unit accepted root with
   no password; that is an observed factory state, not an installer credential
   or a safe deployment default. Do not automate accepting unknown host keys.
3. Save configuration and a board-specific recovery backup privately. Hash
   before/after reads and preserve calibration, bootloader, environment, and
   identity material. Never redistribute a user's backup as a firmware image.
4. Create a new private directory beneath RAM-backed `/tmp` with `umask 077`.
   Stream the binary there; verify its device-side length and SHA-256 against
   the local candidate before making it executable.
5. Launch it with a dedicated state directory in that same RAM directory, an
   explicit wired listen address, and `--run-for 300`. Capture stdout/stderr.
   Keep the PID and verify `/proc/PID/exe` before later signalling it.
6. Require `hopspot_ready`, then run the page probe from the development host.
   A listener or process alone is insufficient. Capture RSS, high-water memory,
   thread count, free RAM, state size, and logs after the conversation.
7. Send SIGTERM to that verified PID. Require final persistence diagnostics and
   `hopspot_stopped`, then confirm process exit. Restart with the same directory;
   require the same destination and another successful page probe.
8. Keep the service in RAM for subsequent radio experiments or stop it and remove
   only the recorded deployment files. Reboot removes both the RAM binary and
   RAM identity. Export private state securely first if its identity must survive.

The development run left a RAM-only service on the wired address at TCP 4242.
It did not install boot scripts, change firewall/radio configuration, reboot,
or write a firmware image. That temporary service is not an installed product.

## Physical results

- Static MIPS executable: approximately 3.2 MiB.
- First page probe: 3,542-byte packed response verified, reported request RTT
  90 ms. This is one wired request, not a throughput or radio benchmark.
- After that request: RSS 3,420 KiB, high-water RSS 3,492 KiB, two threads.
  RAM-backed executable pages contribute to RSS; do not add all RSS and binary
  size as if they were disjoint allocations. This is not a worst-case RAM bound.
- SIGTERM produced both routing-state and ratchet flush diagnostics and a clean
  stop. A process restart retained the destination and served the page again.
- Host tests passed for state locking, CLI requirements, clean restart, and
  refusing a damaged identity. Native Clippy passed with warnings denied.

## What an installer could automate

The existing web flasher is the installation entry point: select this Linux
appliance, obtain a verified application bundle, and follow its installation
guide. Keep the working vendor radio stack and install a versioned Hopspot
service. The first supported transfer may use local SSH tooling; later options
include an authenticated device upload endpoint or a local helper launched from
the web workflow. The browser flasher is not yet a G4 transport or recovery solution.

The installer should own a staged transaction: inspect hardware/ABI/firmware;
verify a signed, compatible artifact; acquire authenticated management access;
back up privately; check space for old and new versions plus state; upload and
hash a candidate; run a bounded health test; install a supervised service; verify
the page after activation; retain a known-good executable and roll back on
failure. Keep identities and configuration outside replaceable application
versions. Interrupted uploads must never become the active executable. Avoid
an endless supervisor restart loop on a corrupt identity or full filesystem.

The slot manager owns application activation; headless owns identities and
retained state. Use a pinned controller to verify build/interface snapshots, an
authenticated app message and the exact node page before confirming a candidate.
Least privilege, log/state growth, power loss during state saves and interrupted
flash writes remain qualification gates. Reboot can interrupt the host's Ethernet
DHCP lease: retain a reviewed static management fallback or allow bounded lease
recovery, while keeping the actual internet interface separate. Do not interpret
that management delay as application startup time.

## What a firmware flasher still needs

Firmware replacement is a separate qualification effort. Obtain reproducible
images, redistribution rights for required vendor components, exact hardware and
flash-layout matching, validated upgrade metadata, and an exercised recovery
path. Prefer preserving bootloader, environment, and factory calibration through
the vendor-supported upgrade path. Never clone a donor device's entire flash
onto another unit.

The saved bootloader contains HTTP/TFTP recovery code, but physical entry timing,
recovery address, serial wiring, accepted image format, and a successful restore
have not been demonstrated. UART evidence disagreed between 57,600 and 115,200
baud. The normal-runtime long button press performs a destructive settings reset;
it is not proof of bootloader recovery. A full raw flash backup is not an image
to upload blindly to LuCI or sysupgrade.

Until those gaps close, a guided, model-specific set of steps with a verified
application installation is more supportable than a one-click firmware promise.
HaLow qualification should separately prove over-air group delivery, suppression
of unwanted unicast replication, peer association behavior, and Reticulum
forwarding across multiple nodes. Wired TCP success establishes none of those.

## Native HaLoW application smoke, 2026-09-29

The G4 ran the `wifi-halow` headless application from RAM, with TCP bound only to
`127.0.0.1:4343`, using device `wlan0`, scope `lab-g4`, a two-peer cap,
three-second announcements, and a 90-second lifetime. The first Heltec ran `fetch_page` with
`--halow-device wlan0 --halow-scope lab-heltec` and the advertised destination.
No TCP target was supplied to the probe. Both device-side SHA-256 values matched
before execution:

- Application: `4a797cc47b35591c32ac7a50576569818fb125ecbe465cedced0c5099dc3352c`.
- Probe: `1982303b56667dc4c9cbcaa3f94e824d59966540b8d7e7543323beec5cfc355a`.

The probe reported `hopspot_page_verified bytes=3542 rtt=RttMillis(73)`. This
verifies discovery, native framed data, a Reticulum link, and exact page content
larger than one HaLoW frame. It is one close-range functional smoke, not a
throughput or reliability measurement. Both Morse health checks passed. The
existing TCP Hopspot process and radio configuration were retained; no flashing
or persistent installation was performed.

The new build task's `--with-probe` option packages both executables and their
hashes. See [the headless README](../README.md#experimental-halow-attachment) for
attachment, interval, scope, and memory-limit semantics. The temporary host
flushed routing/ratchet state and reported `hopspot_stopped` at its deadline;
neither temporary process remained, and wireless UCI changes were empty.
