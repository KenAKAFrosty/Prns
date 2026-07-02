# Interface-crate split — migration plan

Splitting the interface **impls** out of the `personal-rns` engine into per-runtime
crates, so runtime and interface become orthogonal axes and the feature soup dies.
Grounded in a full recon of the current tree (core/impl boundary, trait contract,
dependency graph, feature→dep map, consumer blast radius).

## 1. Target architecture

Three crates, one dependency direction (arrows point at `personal-rns`):

```
personal-rns  (core: pure engine + reactor + runtime binds + ALL traits + ALL wire-cores)
      ▲                                   ▲
      │ [tokio-host]                      │ [embassy-contract]
personal-rns-interfaces/tokio      personal-rns-interfaces/embassy
   (host interface impls)             (embedded interface impls)
```

- `personal-rns-interfaces/` is a **directory**, not a shared workspace: `tokio/` and
  `embassy/` are independent crates built for different targets (host vs bare-metal), each
  its own workspace, both **excluded** from the root workspace exactly like `personal-rns-ffi`
  (to keep their runtime deps out of the engine's `--workspace` feature unification).
- Consumer picks the crate for their runtime (a choice they make anyway) + the interface
  features they want. `personal_rns` core still carries `tokio-host`/`embassy-contract` for the
  reactor + binds, but **no interface pulls a runtime into core anymore** — so the
  `tokio-host`+`embassy-contract` both-on collision (and its `Prns`/`Fleet` guard) **dissolves**.

## 2. The boundary — what stays in core vs moves

### Stays in `personal-rns` (core)
- **Engine** (`engine/`), **reactor** (`reactor/interface_seam.rs`, `reactor/grant.rs`,
  `reactor/mod.rs::Host`, and `reactor/impls/{tokio_reactor,embassy_reactor}.rs`), **runtime
  binds** (`runtime/` — `Prns`/`Fleet`/`InterfaceSupervisor`/`interface_set`/the barrels).
- **Every seam trait** the interface impls are generic over: `Interface`, `InterfaceSeam`,
  `InterfaceStatus`, `Grant{Producer,Consumer}`/`FrameSlot`, `Host`, `InterfaceAttach`/`Set`,
  `ReportsStatus`/`StatusView` (tokio-gated), `ReactorEgress`/`InterfaceLifecycle` (embassy-gated).
- **Every hardware seam trait**: `BleBackend`/`BleLink`/`BleSource`/`BleSink`, `EspNowRadio`,
  the SX1262 `embedded-hal-async` bounds + `Sx126x` driver (`subghz_rf.rs`), `PeerStore`,
  `RpcQuerySource`, the `embedded_io_async`/`tokio::io` stream boundaries.
- **All wire-cores** (the runtime-agnostic `*/core.rs`) + the framing codecs
  (`framing/{kiss_framing,rns_serial_framing}.rs` + `FrameBuffer`) + the shared legacy
  `impls/usb_auto/core.rs` + `status.rs` (agnostic types) + `substrate/` (embassy timebase).
- **Agnostic-file-under-impls exception:** `local/impls/rpc_value.rs` (pure msgpack) → relocate
  to a core-appropriate spot.

### Moves to `personal-rns-interfaces/tokio`
The tokio impls, incl. the tokio-by-content files that don't follow the `tokio.rs` naming:
- `*/impls/tokio.rs` (udp, serial, kiss, ax25_kiss, rnode, pipe, usb_auto, bluetooth_auto),
  `tcp/{client,server}/tokio.rs`, `tcp/tokio_socket.rs`, `websocket/{tokio_wire,client/tokio,server/tokio}.rs`,
  `wifi_auto/impls/tokio.rs`, `local/impls/{tokio.rs,rpc_compat.rs}`,
  `bluetooth_auto/impls/bluer.rs`, **`backbone/{client,server}.rs`**, **`framed_stream.rs`**
  (the tokio shared serve-loop + its `Framing`/`StreamDeframer` markers).
- Intra-crate tokio→tokio couplings (`wifi_auto→tcp`, `ax25_kiss→kiss`, `backbone→tcp::tokio_socket`)
  all live here together — the split never severs them.

### Moves to `personal-rns-interfaces/embassy`
- `*/impls/embassy.rs` (usb_auto, bluetooth_auto, wifi_auto), `tcp/client/embassy.rs`,
  `lora/impls/embassy.rs`, `esp_now/impls/embassy.rs`, `usb_auto/impls/embassy_usb.rs`.
- **Zero embassy→embassy cross-family coupling** (verified) — clean.

### The one straddle file
`bluetooth_auto/seam.rs` = agnostic `BleBackend`/`BleLink`/… traits **+** an `embassy-seam`-gated
tail (`embassy_sync` signal). Split it: agnostic traits → core; the embassy-seam tail →
`interfaces/embassy` (or keep in core behind `embassy-seam`; decide in Phase 0).

## 3. The feature model

**Core has NO per-interface features.** The traits and wire-cores are **always present**:
trait/type defs are zero-cost until used; the wire-cores are dep-free (pure logic over the
always-on `crypto`/`heapless`), so always-compiling them pulls nothing extra; unused wire-core
functions are stripped from the final embedded binary by `--release` + LTO + `--gc-sections`.
Only residual cost is marginal compile time. Posture: always-present by default — gate an
individual core later *only if* Phase 0 shows one is heavy or dep-pulling. Core keeps just the
**runtime** features (`tokio-host`/`embassy-contract`), allocation (`alloc`/`std`/`external-alloc`),
and capabilities (`stream-compression`, …).

**Each interface crate** depends on core with just its **runtime** feature, and carries the
interface features — which pull ONLY the runtime-specific external deps (no core-feature ref;
the conjunction problem never arises because the runtime is fixed by the crate):

```toml
# prns-interfaces-tokio   (dep: personal-rns = { default-features = false, features = ["tokio-host"] })
tcp   = ["dep:socket2", "tokio/net"]
udp   = ["tokio/net"]
wifi  = ["tcp", "dep:if-addrs", "dep:netdev"]
local = ["tokio/net", "dep:md-5"]
websocket = ["dep:tokio-tungstenite", "tokio/net"]
ble   = ["dep:bluer", "dep:personal-rns-ffi"]   # OS-split below
# no esp-now, no lora — don't exist on tokio

# prns-interfaces-embassy   (dep: personal-rns = { default-features = false, features = ["embassy-contract"] })
tcp     = ["dep:embassy-net"]
wifi    = ["dep:embassy-net"]
lora    = []                    # SX1262 seam, board-provided — pure compile toggle
esp-now = []                    # EspNowRadio seam, board-provided
ble     = ["dep:trouble-host"]  # HCI seam, board-provided
usb     = []
# no bluer, no host TCP server — don't exist on embassy
```

Interface features are the `#[cfg(feature = "…")]` toggles that select which impl module compiles;
some embassy ones are `[]` (the impl compiles, its hardware deps come from the board's seam impl).

**OS backends within a crate** — proven pattern (`bluer` already uses it today):
```toml
[target.'cfg(target_os = "linux")'.dependencies]
bluer            = { optional = true }
[target.'cfg(any(target_os = "macos", target_os = "windows"))'.dependencies]
personal-rns-ffi = { optional = true }
```
`ble` on → Linux gets `bluer`, mac/win get the FFI CoreBluetooth/WinRT backend, off → neither.
Verified: a feature referencing a wrong-OS optional dep does **not** error.

This retires the soup: `tcp` no longer implies `tokio-host`; embedded TCP leaves `embassy-wifi`;
`embassy-lora`/`-espnow`/`-bluetooth` become plain `lora`/`esp-now`/`ble` on the embassy crate.

## 4. Consumer migration surface

Rewrite `personal_rns::interfaces::<fam>::impls::<runtime>::X` →
`<new-crate>::<fam>::X`, and add the new crate dep + interface features. **`…::core` /
type-only imports stay pointed at `personal-rns`** (cores never leave core), so pure-core
consumers barely change.

- **Tokio consumers (touch a lot):** `personal-rnsd` (11 families — widest), `personal-hopspot/desktop`
  (BLE per-OS blocks, usb_auto, wifi_auto, local, tcp), `mobile/{ios,android}`, `benchmarks`,
  `personal-rns-ffi` (implements `BleBackend` — keeps depending on core for the seam; the tokio
  `BluetoothAuto` that consumes it moves to `interfaces/tokio`).
- **Embassy consumers:** `embedded/esp32` (S3: wifi/lora/esp-now/usb/ble/tcp; C6: usb/esp-now/ble),
  `embedded/nrf52840` (lora/usb/ble).
- **Barely-change (core/types only):** `personal-rns-wasm`, `personal-rns-config`,
  `personal-hopspot/core`, `fuzz` (uses `local::impls::rpc_value::Value` — relocated with the
  agnostic file).
- **Cleanups to fold in:** the two aliased USB-core paths (`usb_auto::core` vs
  `impls::usb_auto::core`, both live in wasm + ios) collapse to one canonical path; the
  `bluetooth_auto`/`wifi_auto` root re-exports (`BluetoothAuto`/`AutoWifi`) move to the interface
  crate while their `core`/`seam`/`limits` stay in core — consumers that mix both must split imports.

## 5. Sequencing (green + all-platform-proven)

Decision: **one fell swoop** for the move itself — no incremental family-by-family, no pilot.
Cleaner history, consumers touched once, old code deleted once. Core-prep stays a separate
pre-step; the move is one coordinated change; polish follows.
**Standard: a phase is "done" only when green on every real target it touches — not the convenient one.**

- **Phase 0 — Core prep (no new crates, no behavior change).** Make the surface the interface crates
  build against externally reachable (traits + wire-cores `pub`, not `pub(crate)`); split the
  `seam.rs` straddle; relocate `rpc_value.rs`; confirm every `*/core.rs` compiles always (dep-free/
  no_std) — gate an exception only if one proves it needs it. No `*-core` features. (Barrel work
  already reverted — revisit post-move.)
  **Gate:** core builds every arm (default, tokio-host, embassy-contract, embassy-wifi/lora/espnow/bluetooth).
- **Phase 1 — The move (one swoop).** Stand up `prns-interfaces/{tokio,embassy}`; relocate every
  tokio impl → the tokio crate and every embassy impl → the embassy crate; migrate every consumer
  (rnsd, desktop, ios, android, benchmarks, ffi, esp32, nrf52840); retire the feature soup
  (agnostic feature names, drop `tcp⇒tokio-host`, rename the embedded features); delete the old
  impl locations. **Gate — the full all-platform matrix:** host (rnsd/desktop/benchmarks/ios/android)
  + esp32-S3 (xtensa) + esp32-C6 (riscv) + nrf52840 (thumbv7em), all green.
- **Phase 2 — Polish.** Prelude + docs updated to the new crate paths; CI build jobs + fmt-check
  wired for both new crates; the all-platform matrix becomes a required gate.

## 6. CI & gates
- New standalone crates need build jobs: `interfaces/tokio` on host; `interfaces/embassy`
  cross-built for xtensa (esp32-s3), riscv32 (esp32-c6), thumbv7em (nrf52840).
- Add both crate dirs to `scripts/fmt-check.sh` (the per-workspace fmt loop).
- The all-platform build matrix becomes a required gate (Phase 5), per the "proven everywhere" standard.

## 7. Decisions (locked) + one open detail
1. **Crate names:** `prns-interfaces-tokio` / `prns-interfaces-embassy` (dir `prns-interfaces/{tokio,embassy}`).
2. **Interface feature vocabulary:** `ble` / `wifi` / `esp-now` / `usb` / `lora` — confirmed.
3. **Granularity:** one fell swoop (§5) — not family-by-family, no pilot.
4. **`-ffi` coupling:** the `ble` tokio feature **depends on `personal-rns-ffi`** (mac/win backend).
   Interfaces are the "meets-the-real-world" layer, live outside the `forbid(unsafe)` core, so some
   ffi there is clean; the inverse buys only confusion.
5. **Barrel work:** reverted; revisit after the move.
6. *(open, decide in Phase 0)* **The `seam.rs` straddle** destination — agnostic traits → core;
   the `embassy-seam` tail → core-behind-`embassy-seam` vs the embassy crate.

## 8. Related follow-on (tracked; NOT part of this split): extract platform BLE backends

The `BleBackend` seam drivers are misfiled. macOS/Windows live in `personal-rns-ffi` and Linux in
the interface layer (correct — reusable library code), but the **Android (805 loc), esp32 (1350 loc),
and nrf52840 (1549 loc)** drivers live inside the Hopspot app — ~3,700 lines of reusable platform
driver code stranded in an app. Target is the three-layer model: interface impl (`BluetoothAuto<B>`,
backend-agnostic) → backend driver (platform/board crate) → app (wires them). Extract the Android JNI
driver into a host-backend crate (beside the `-ffi` mac/win backends) and the esp32/nrf drivers into
board crates. Distinct from this split (that moves the *supervisor*; this moves the *backends*), and
the app files mix driver + wiring, so it's an extraction refactor, not a move. **Do after the split.**
