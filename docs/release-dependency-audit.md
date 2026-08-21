# Public-Release Dependency Audit

This is the release policy and evidence map for the shipped Prns engine, daemon, desktop/mobile
apps, firmware, WebAssembly module, and npm package. The per-build evidence artifact records the exact Git
commit and hashes of the checked baselines.

## Reproducible graph matrix

| Graph | Manifest | Shipped target |
|---|---|---|
| Engine | `Cargo.toml` | `x86_64-unknown-linux-gnu` |
| Daemon | `prnsd/Cargo.toml` | Linux, macOS, Windows |
| Desktop | `personal-hopspot/desktop/Cargo.toml` | Linux, macOS, Windows |
| Android | `personal-hopspot/mobile/android/rust/Cargo.toml` | `aarch64-linux-android` |
| iOS | `personal-hopspot/mobile/ios/rust/Cargo.toml` | `aarch64-apple-ios` |
| nRF52840 | `personal-hopspot/embedded/nrf52840/Cargo.toml` | `thumbv7em-none-eabihf` |
| ESP32-C6 | XIAO board manifest | `riscv32imac-unknown-none-elf` |
| ESP32-S3 | Heltec V4 and T-Beam board manifests | `xtensa-esp32s3-none-elf` |
| WebAssembly/npm | `prns-wasm/Cargo.toml` / `package-lock.json` | `wasm32-unknown-unknown` |
| Hosted website | `docs/website/Cargo.toml` / `package-lock.json` | Rust/WebAssembly plus bundled browser JavaScript |
| Standalone flasher | `personal-hopspot/flasher/Cargo.toml` | macOS arm64/x86_64, Linux arm64/x86_64, Windows x86_64 |

`validation/security/deps-audit.sh` runs this matrix with `--locked`, excludes non-shipped development
dependencies, and checks advisories, licenses, sources, and bans with cargo-deny 0.19.8.

## Resolved release blockers

- RUSTSEC-2026-0204 is resolved by `crossbeam-epoch 0.9.20`.
- RUSTSEC-2026-0194 and RUSTSEC-2026-0195 are resolved by `plist 1.10.0` and `quick-xml 0.41.0`.
- `dirs`/`option-ext`, `serialport`, and `tokio-serial` were removed from the engine/application
  graphs. The standalone hardware flasher intentionally carries exact-scoped `directories` and
  `serialport` dependencies for its user cache and direct USB operation; that separate graph is
  audited on every published operating system target.
- Linux does not instantiate tray-icon's GTK3/GLib path for either `prnsd` or the Hopspot desktop
  face. Both use the blocking StatusNotifier backend in `ksni 0.3.6`; tray-icon remains
  target-scoped to macOS and Windows.

The allowlist in `deny.toml` is a permissive-by-default policy: every unlisted expression fails.
GPL, LGPL, AGPL, and unknown licenses are not accepted. Package-scoped additions are `ksni 0.3.6`
under Unlicense, `nrf-softdevice-s140 0.1.2` under the hash-pinned Nordic terms, and the exact
`serialport 4.9.0` transport required by the mandated `espflash 4.5.0` library under file-level
MPL-2.0. That narrow hardware boundary is shipped with its source/license notice; MPL is not
accepted generally. The SoftDevice source remains restricted to revision
`47d6121c6e823120e8b883a7ac75f44ce7daa3aa`.

## Unsafe enforcement

Every shipped first-party Rust target must declare `#![forbid(unsafe_code)]` unless its package is
one of these reviewed boundaries:

- `prns-core`, `prns-runtime`, and `prns-runtime-embassy`: documented field-by-field in-place
  initialization that keeps the large engine and node values off constrained embedded stacks.
- `prns-ffi`: Objective-C, IOKit, WinRT, SetupAPI, and Windows COM handles.
- `personal-hopspot-android`: JNI pointers and Java-owned buffers.
- `personal-hopspot-ios`: the exported C ABI and caller-owned framebuffer.
- `t-echo`: SoftDevice SVCs and the fixed L2CAP packet pool.
- `personal-hopspot-esp32`: ROM calls, reserved-memory registration, and persistent RTC state.

Each exception denies `unsafe_op_in_unsafe_fn` and undocumented unsafe blocks. The deterministic
`validation/security/unsafe-audit.py` combines locked Cargo metadata with a nested-comment/string/raw-string-
aware Rust token scan. `audits/unsafe-snapshot.json` records package version, source, enabled
features, graph membership, and unsafe token classes; unexplained drift fails CI.

Cargo-geiger 0.13.0 remains supplemental evidence only. Its logs are always uploaded, and parser
failure is reported as an incomplete inventory rather than accepted as a green gate. The compiler
forbids and the reviewed metadata/token snapshot are the enforcement mechanisms.

## Notices and non-Cargo graphs

`THIRD_PARTY_NOTICES.md` is generated and byte-checked with cargo-about 0.9.1. Each complete locked
manifest closure is fetched first; cargo-about then target-filters and reads only the packaged
license material in offline mode, so mutable network responses cannot change signed release
evidence. The bundle canonicalizes presentation-only whitespace (line endings, trailing space, and
repeated blank lines) without changing legal words, then deduplicates those texts across the graph
matrix. It includes bundled native-code notices, reproduces the Nordic terms, and appends the
checked Android Maven and hosted JavaScript closures. The browser graph pins
`esptool-js 0.6.0`, `spark-md5 3.0.2` (MIT alternative), and their exact transitive runtime graph.
Playwright 1.61.1 and axe 4.12.1 are exact-pinned development-only browser/accessibility tools.
Their locked registry sources, integrity hashes, and permissive/MPL licenses are audited, while
production source and bundle scans prevent them from entering shipped output; because they are not
distributed, they are intentionally absent from the shipped notice bundle.
Firmware builds copy the notices beside every hosted image; CLI archives and the website carry the
same checked bundle.

Android's `releaseRuntimeClasspath` must exactly match
`personal-hopspot/mobile/android/dependencies/release-runtime.tsv`. The npm production graph must
remain empty for `prns-wasm`; the website graph must exactly match the explicit browser-flasher
allowlist and cannot contain a runtime CDN or the legacy ESP Web Tools engine.

## Pinned tools and remaining physical checks

- cargo-deny 0.19.8
- cargo-about 0.9.1
- cargo-geiger 0.13.0 (advisory evidence)

OS/hardware acceptance—including the complete web/CLI all-board matrix and every published CLI
architecture—must be signed off against the exact signed candidate. The protected promotion job
validates `acceptance.json`; those physical observations cannot be inferred from CI.
