# Public-Release Dependency Audit

This is the release policy and evidence map for the shipped Prns engine, daemon, desktop/mobile
apps, firmware, WASM module, and npm package. The per-build evidence artifact records the exact Git
commit and hashes of the checked baselines.

## Reproducible graph matrix

| Graph | Manifest | Shipped target |
|---|---|---|
| Engine | `Cargo.toml` | `x86_64-unknown-linux-gnu` |
| Daemon | `prnsd/Cargo.toml` | `x86_64-unknown-linux-gnu` |
| Desktop | `personal-hopspot/desktop/Cargo.toml` | Linux, macOS, Windows |
| Android | `personal-hopspot/mobile/android/rust/Cargo.toml` | `aarch64-linux-android` |
| iOS | `personal-hopspot/mobile/ios/rust/Cargo.toml` | `aarch64-apple-ios` |
| nRF52840 | `personal-hopspot/embedded/nrf52840/Cargo.toml` | `thumbv7em-none-eabihf` |
| ESP32-C6 | XIAO board manifest | `riscv32imac-unknown-none-elf` |
| ESP32-S3 | Heltec V4 and T-Beam board manifests | `xtensa-esp32s3-none-elf` |
| WASM/npm | `prns-wasm/Cargo.toml` / `package-lock.json` | `wasm32-unknown-unknown` |

`scripts/deps-audit.sh` runs this matrix with `--locked`, excludes non-shipped development
dependencies, and checks advisories, licenses, sources, and bans with cargo-deny 0.19.8.

## Resolved release blockers

- RUSTSEC-2026-0204 is resolved by `crossbeam-epoch 0.9.20`.
- RUSTSEC-2026-0194 and RUSTSEC-2026-0195 are resolved by `plist 1.10.0` and `quick-xml 0.41.0`.
- `dirs`/`option-ext`, `serialport`, and `tokio-serial` were removed from the shipped graphs. The
  replacement is `home 0.5.12`, a Prns-owned path policy, `serial2-tokio 0.1.24` on Unix, and native
  single-open/enumeration implementations quarantined in `prns-ffi`.
- Linux no longer instantiates tray-icon's GTK3/GLib path. It uses the blocking StatusNotifier
  backend in `ksni 0.3.6`; tray-icon remains target-scoped to macOS and Windows.

The allowlist in `deny.toml` is a literal zero-copyleft policy: every unlisted expression fails.
MPL, GPL, LGPL, AGPL, and unknown licenses are not accepted. The only package-scoped additions are
`ksni 0.3.6` under Unlicense and `nrf-softdevice-s140 0.1.2` under the hash-pinned Nordic terms.
The SoftDevice source is restricted to revision `47d6121c6e823120e8b883a7ac75f44ce7daa3aa`.

## Unsafe enforcement

Every shipped first-party Rust target must declare `#![forbid(unsafe_code)]` unless its package is
one of these reviewed boundaries:

- `prns-ffi`: Objective-C, IOKit, WinRT, SetupAPI, and Windows COM handles.
- `personal-hopspot-android`: JNI pointers and Java-owned buffers.
- `personal-hopspot-ios`: the exported C ABI and caller-owned framebuffer.
- `t-echo`: SoftDevice SVCs and the fixed L2CAP packet pool.
- `personal-hopspot-esp32`: ROM calls, reserved-memory registration, and persistent RTC state.

Each exception denies `unsafe_op_in_unsafe_fn` and undocumented unsafe blocks. The deterministic
`scripts/unsafe-audit.py` combines locked Cargo metadata with a nested-comment/string/raw-string-
aware Rust token scan. `audits/unsafe-snapshot.json` records package version, source, enabled
features, graph membership, and unsafe token classes; unexplained drift fails CI.

Cargo-geiger 0.13.0 remains supplemental evidence only. Its logs are always uploaded, and parser
failure is reported as an incomplete inventory rather than accepted as a green gate. The compiler
forbids and the reviewed metadata/token snapshot are the enforcement mechanisms.

## Notices and non-Cargo graphs

`THIRD_PARTY_NOTICES.md` is generated and byte-checked with cargo-about 0.9.1. It deduplicates exact
license texts across the graph matrix, includes bundled native-code notices, reproduces the Nordic
terms, and appends the checked Android Maven coordinates. Firmware builds copy it beside every
hosted image; `scripts/stage-release-notices.sh` attaches it to binary/app distribution directories.

Android's `releaseRuntimeClasspath` must exactly match
`personal-hopspot/mobile/android/dependencies/release-runtime.tsv`. The npm production graph must
remain empty; TypeScript 5.9.3 is recorded only as non-shipped development tooling.

## Pinned tools and remaining physical checks

- cargo-deny 0.19.8
- cargo-about 0.9.1
- cargo-geiger 0.13.0 (advisory evidence)

OS/hardware acceptance—Windows ESP32/RNode single-open behavior, USB-only discovery on all three
desktop OSes, GNOME/KDE tray interaction, and physical nRF/ESP target smoke—must be signed off on
release hardware. Those physical observations cannot be inferred from CI and are recorded
separately from the automated audit result.
