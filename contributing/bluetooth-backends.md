# Bluetooth backends: Linux and Windows

A handoff for the two desktop backends, written from the macOS bring-up so you
don't re-pay the tuition. The native Reticulum-over-BLE transport is GATT-first
and radio-proven on macOS <-> Android. Three backends exist today: macOS
(`personal-rns-ffi/src/ble/macos.rs`), Android
(`personal-hopspot/platform_impls/android/rust/src/ble.rs`), and Linux `bluer`
(`personal-rns/src/interfaces/bluetooth_auto/impls/bluer.rs`, **stale** against
the current seam, see below). Windows is greenfield.

The shared brain (discovery dedup, orientation, make-before-break) lives in the
supervisor. A backend only drives the radio and the seam. Mirror an existing
backend rather than inventing structure.

## The model in one breath

The link **is** the GATT control connection plus a GATT-data floor. It is
present the moment the handshake settles and gone only when the control
connection drops. L2CAP is an **optional per-frame fast lane**, never the link
itself. The receiver is omnivorous (frames arrive on either plane into one
ingress). Interface presence is independent of L2CAP. Never tear a working GATT
link to chase the L2CAP upgrade.

## The seam you implement

`personal-rns/src/interfaces/bluetooth_auto/seam.rs`:

- `BleBackend`: `MAX_PEERS`, `set_advertising(bool)`, `next_event() -> BleEvent`,
  `dial(BleAddress)` (fire-and-forget), `on_link_closed(BleAddress)`.
- `BleLink`: `dialect()`, `address()`, `control_send(&Control)`,
  `control_recv()`, `upgrade(&L2capPlan)` (fire-and-forget, **non-fatal**),
  `into_data() -> (Source, Sink)`.
- `BleSource::recv_frame(&mut [u8]) -> usize`, `BleSink::send_frame(&[u8])`.

You surface `BleEvent::{Sighting, Inbound(link), LinkReady { link, origin,
peer_rssi }}`; the supervisor does identity-keyed dedup and make-before-break.
Do not dedup in the backend.

## Discovery and the control wire (`core.rs`)

- Service UUID `…28e3` (shared with Columba / ble-reticulum). The core helper
  emits advertise flags + the 128-bit UUID little-endian; Android may also add
  ble-reticulum v0.3 manufacturer data (`0xFFFF`, version `0x03`, dual-mode
  flags `0x00`). `encode_advertisement()` and `contains_service()` give you the
  bytes for the core advertisement; don't hand-roll the AD structure.
- Native control characteristic `…28e7` (write + notify); native data floor
  characteristic `…28e8` (write + notify). The control PDU is `CONTROL_MAX_LEN`
  (23 bytes). `Control::encode`/`decode` does the framing; carry the bytes.
- GATT-data floor fragmentation: the 5-byte header (`FRAGMENT_HEADER_LEN`),
  `fragments_of` / `Reassembler` (same shape as Columba).
- L2CAP framing: a 2-byte big-endian length prefix per frame
  (`encode_stream_frame` / `StreamDeframer`). L2CAP is a byte stream
  cross-platform, not a SeqPacket; don't lean on SDU boundaries.

## Orientation: who opens L2CAP (`core.rs` `arrangement()`)

`Endpoint` is stack-first and nested: `BlueZ(BlueZHost)`, `WinRt(WinRtHost)`,
etc. `arrangement(local, peer)` returns `GattOnly | EitherOpens | Opens(ep)`; an
**unknown pair is `GattOnly`**, the always-works floor. Promote a pair to
`Opens(...)` only after radio-watching that exact pair.

- **Windows is always `GattOnly`.** WinRT exposes zero app-level L2CAP. Don't
  implement `upgrade()` past a no-op. The floor carries every frame; this is a
  complete, correct backend, not a degraded one.
- **Linux can open and accept L2CAP.** macOS is acceptor-only (a Mac central
  open bonds), so `Mac <-> Linux` is seeded `Opens(Linux)`: Linux opens toward
  the Mac's listener.

## The macOS learnings that will save you days

1. **Pairing prompt.** BLE L2CAP security is set by the **listener**. A central
   opening toward a secured listener triggers a bond/prompt. Keep the listener
   insecure (`BT_SECURITY_LOW` on bluer; `listenUsingInsecureL2capChannel` on
   Android) and only open from the side `arrangement()` names.
2. **BR/EDR bearer (Linux-specific, already root-caused).** A dual-mode Mac
   advertises "Simultaneous LE + BR/EDR" with a public address; BlueZ
   `Device.Connect` then falls to the **classic** bearer and triggers Secure
   Simple Pairing, a separate layer from the LE CoC security. Fix:
   `Adapter.ConnectDevice` with an explicit LE `AddressType` (and remove the
   cached device first), and pin the discovery filter to `transport=le`. A stale
   BlueZ cache defeats the scan filter: `bluetoothctl remove <addr>`.
3. **ATT MTU.** Negotiate it up (request 517) **before** relying on the 23-byte
   control PDU or multi-byte data fragments; the default 20-byte ATT payload
   silently truncates them. The side that initiates discovery requests the MTU
   and waits for the changed-callback before discovering characteristics.
4. **Advertising overflow.** Don't let the OS pad the advert with a long host
   name. Set a short local name so the 128-bit service UUID stays in the primary
   (peer-visible) packet rather than the overflow region.
5. **Retain the channel object.** Hold the L2CAP channel for the data plane's
   whole life, not just its stream halves; the OS deallocates it otherwise and
   resets the connection.
6. **Omnivorous receiver, fire-and-forget upgrade.** Stand up the GATT floor
   immediately in `into_data()` and fan both planes into one frame channel.
   `upgrade(&L2capPlan)` arms L2CAP in the background and can never fail the
   link; on an L2CAP send error, drop to the floor frame-by-frame.

## Where it lives and the gate

- **Linux `bluer` stays in `personal-rns`** (bluer is a safe crate, feature
  `bluetooth-bluer`). **First task: re-adopt `bluer.rs` to the current seam.** It
  still references the deleted `Transport` type (now `L2capPlan`) and
  `advertise()` (now `set_advertising(bool)`), so it does not compile against
  `main`. No CI lane currently builds it, which is why the drift went unseen; add
  one (`cargo check --no-default-features --features bluetooth-bluer` on a Linux
  runner) so it cannot silently rot again.
- **Windows is `windows-rs` / WinRT, GATT-only.** `GattServiceProvider` for the
  peripheral + advertise role, a GATT client for central; note that
  `GattServiceProvider` advertising and the custom beacon publisher are mutually
  exclusive. Any unsafe FFI goes in `personal-rns-ffi` alongside macOS (the
  engine is `#![forbid(unsafe_code)]`); audit with cargo-geiger + Miri per the
  repo rules.
- **Reference impls to mirror:** `personal-rns-ffi/src/ble/macos.rs` (the
  Listener/Central `GattLink` split, the floor-first `into_data`, the per-frame
  lane pick) and `personal-hopspot/platform_impls/android/rust/src/ble.rs`.
