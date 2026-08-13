# Prns Hopspot: headless configuration (webUI as a virtual screen)

Design for configuring a **headless** Personal Hopspot — one with no display, no
button, no keyboard, USB/BLE only (the Seeed SenseCAP T1000-E is the first; a
headless XIAO ESP32-C6 is another). The goal: a **webUI that replaces the
screen**, with **full parity** against every setting the screen-equipped boards
(the LilyGo T-Echo e-ink UI) expose. The webUI is a first-class config *face*,
not a separate tool.

Companion to `T1000E_HOPSPOT_PORT.md`. Scope: the config path. The T1000-E port
itself (LR1110 driver, board variant, `Radio` trait) is in PR #107; flashing +
boot on real T1000-E hardware is verified (see "Flash transport" below).

Status: design pass. Implementation phasing at the end. The shape of the
device-side action dispatch extraction (section "Architecture") needs maintainer
buy-in — it touches the shared nRF52840 runtime the T-Echo also uses.

## The problem

A T1000-E runs the same Prns firmware as a T-Echo, but its render loop is inert
(`DisplayHardware::driver = None` → `firmware.rs` matches `eink: None` to
`core::future::pending().await`). Every user-facing action in Prns is dispatched
from inside that render future. So on a headless board **no user action can
fire** — there is no input device and no screen to render the result.

Today this means a headless T1000-E is fixed at build-time radio defaults
(`DEFAULT_915_PROFILE`: US915 / SF9 / BW250 / CR4_5 / 22 dBm / preamble 18) with
a first-boot-generated identity, and the only lever is rebuild-and-reflash. The
flash-backed `RadioProfileStore` is wired and writable
(`personal-hopspot/core/src/radio_profile_store.rs`) — there is simply no input
path to it on a headless board.

The USB interface today is a **data transport only**. `prns-core`'s WebUSB
protocol (`prns-core/src/interfaces/usb_auto/protocol.rs:28-33`) has three
message kinds — `Hello` / `HelloAck` / `Data` — and `Data` carries Reticulum
packets. There is no config command. (BLE is the same shape: the SoftDevice
GATT server in `runtime/bluetooth_auto.rs` carries the Reticulum peer protocol,
not config.)

## Principle: the webUI is a virtual screen, not a parallel protocol

The T-Echo e-ink UI is already a clean input→action→state→render pipeline:

- Input (button) decodes to a `UiAction`
  (`personal-hopspot/core/src/screen/state/mod.rs:60-74`).
- `UiAction` dispatch (`firmware.rs:447-568`) calls the authoritative apply
  primitives: `LORA_CONTROL.apply(profile)` for radio changes,
  `lora_profile_store.save(profile)` / `reset()` for persistence,
  `*STATUS.toggle_enabled()` for interface on/off, `AnnounceNow` for announce.
- State updates emit `UiNotice`s (`state/mod.rs:77-95`).
- The render future draws `Card`s / spectrum / peers / limits from the same
  state model.

The webUI reuses **all of that**. It is a remote screen: it sends `UiAction`s
over USB (or BLE), and it receives serialized state snapshots (the same
`Card`/`UiNotice`/spectrum/limits data the e-ink renderer consumes). No second
config protocol, no second persistence path, no second action vocabulary. One
authoritative home per concept — per `CONTRIBUTING.md`.

The only new device-side work is:

1. **A config task that calls the existing apply primitives directly.** The
   heavy primitive — `apply_and_persist_radio_profile`
   (`personal-hopspot/core/src/screen/state/mod.rs:120`, re-exported from
   `personal-hopspot-core`) — is *already* a shared pub fn the render loop calls.
   The interface toggles (`*STATUS.toggle_enabled()`,
   `BluetoothAutoStatus::new(&BLE_SHARED).toggle_enabled()`) and announce
   (`ui_handle.issue(AnnounceNow)`) are one-liners against runtime statics
   (`LORA_CONTROL`, `BLE_SHARED`, `LORA_STATUS`/`USB_STATUS`, `ui_handle`). A
   config embassy task spawned in `run()` captures references to those same
   primitives and drives them from `ConfigRequest`s — **without touching the
   render loop's `UiAction` dispatch**. The T-Echo render future is unchanged;
   zero blast radius on the shared runtime. (This dissolves the earlier
   "extract `apply_ui_action`" idea — the apply primitive is already shared, so
   no extraction is needed; only the dispatch *glue* differs, which is
   necessarily input-device-specific. The authoritative home for each action is
   the primitive it calls, not a shared dispatcher.)
2. **A config lane** on the existing WebUSB device: new message kinds that carry
   a serialized `ConfigAction` request and serialized state-snapshot responses.
   The `Data` lane (Reticulum packets) is untouched.
3. **A state-snapshot serializer** for the read-only panels (section "Read-only
   surface"). Reuses the same model types the renderer reads.

> **Why `ConfigAction`, not `UiAction`, over the wire.** `UiAction` is the
> button-input vocabulary and is UI-context-dependent — notably
> `ToggleSelectedInterface` means "toggle the currently-selected card", which
> has no meaning without a screen selection. The config lane needs an
> input-device-independent vocabulary: `ToggleInterface(InterfaceKind)` names the
> interface (LoRa/USB/BLE) explicitly. So the wire carries a `ConfigAction` enum
> (set/toggle LoRa profile, toggle a named interface, sleep/wake, announce), and
> the config task maps each `ConfigAction` to the same primitive the
> corresponding `UiAction` variant calls. Two input vocabularies, one set of
> apply primitives — the primitives are the single authoritative home.

The host side (CLI + webUI) speaks that lane.

## Authoritative settings surface (T-Echo parity spec)

This is the complete set the webUI must expose, derived from the T-Echo UI
(`screen/state/mod.rs`, `screen/state/lora.rs`, `firmware.rs:447-568`).

### Settable — persisted (`RadioProfileStore`, survives reboot)

The LoRa radio profile, `RadioProfile`
(`prns-core/src/interfaces/lora/profile.rs:330-337`):

| Field | Type | Range / options | T-Echo editor |
|---|---|---|---|
| `region` | `Region` (12) | `Us915, Au915, Eu433, Eu865, Eu868, Eu869, As923, In865, Cn470, Kr920, Jp920, Unlimited` ("Custom") — `profile.rs:56-69` | Region picker |
| `frequency` | `Frequency(u32)` Hz | clamped to `[region.band().0, region.band().1]` (`profile.rs:87-102`) | Channel index or MHz+kHz digits |
| `modulation.spreading_factor` | `SpreadingFactor` | `Sf5..=Sf12` (`modulation.rs:5-14`) | Custom row |
| `modulation.bandwidth` | `LoraBandwidth` | `Bw125kHz, Bw250kHz, Bw500kHz` (`modulation.rs:60-64`) | Custom row |
| `modulation.coding_rate` | `CodingRate` | `Cr45, Cr46, Cr47, Cr48` (`modulation.rs:85-91`) | Custom row |
| `modulation` (preset) | `ModemPreset` | `ShortFast, MediumFast, LongFast, LongSlow` (`profile.rs:273-279`) | Preset picker |
| `tx_power` | `TxPower(i8)` dBm | `-9..=22` PA range, further capped per `Region::max_tx_power()` (`profile.rs:122-131`) | Custom row |
| `preamble` | `PreambleSymbols(u16)` | `> 0` (`profile.rs:368-370`) | **not editable on T-Echo** (no editor row, `state/lora.rs:71-91`) |

Validation: `RadioProfile::validate` (`profile.rs:341-372`) — frequency-in-band,
tx-power-in-PA-range, tx-power-under-region-cap, non-zero preamble.

Apply + persist path: `apply_and_persist_radio_profile`
(`personal-hopspot/core/src/screen/state/mod.rs:120-136`) →
`LORA_CONTROL.apply(profile).await` (`prns-interfaces/.../lora.rs:493-539`) →
`lora_profile_store.save(profile).await` / `reset().await`.

Plus the reset action: `UiAction::ResetLoRaProfile` → apply
`DEFAULT_915_PROFILE` + `store.reset()` (writes a "follows default" marker).

> **webUI opportunity (parity+, not parity−):** the T-Echo editor has no
> preamble row (screen space). The webUI has no such limit, so preamble **should
> be editable** on the webUI — it is a real `RadioProfile` field with a
> validator. This is the one place the webUI intentionally exceeds the screen,
> and it is justified (the field exists; only the 1.54" panel could not show it).

### Settable — ephemeral (RAM signals, reset on reboot)

| Setting | Action | Backing | Source |
|---|---|---|---|
| LoRa interface on/off | `ToggleSelectedInterface` | `LORA_STATUS` (`EmbassyInterfaceStatus`) | `firmware.rs:184-186, 483-494` |
| USB interface on/off | `ToggleSelectedInterface` | `USB_STATUS` | `firmware.rs:207-211, 495-506` |
| BLE interface on/off | `ToggleSelectedInterface` | `BLE_SHARED` (`BluetoothAutoStatus`) | `firmware.rs:507-520`; `prns-interfaces/.../bluetooth_auto/runtime.rs:294-314` |
| Sleep (all interfaces off) | `Sleep` | `UiMode::Sleeping` | `firmware.rs:447-457` |
| Wake (all interfaces on) | `Wake` | — | `firmware.rs:458-468` |
| Announce now | `Announce` → `PrnsCommand::AnnounceNow` | one-shot | `firmware.rs:469-480` |

These do **not** persist; reboot re-enables all interfaces. The webUI should
reflect current runtime state and let the user toggle, but should NOT pretend
these are stored preferences.

### Read-only surface (status panels)

The webUI mirrors what the e-ink cards/menus show, read from the same model:

- **Title bar**: battery (`BatteryGauge::lipo()`, `personal-hopspot/core/src/battery.rs`).
- **Per-interface cards** (`screen/model.rs:340-355`): connection state
  (`ConnectionState` Init/Live/Degr/Retry/Fail/Disc/Off/Unkn — `model.rs:320-331`),
  failure reason, cumulative TX/RX bytes, peers/destinations count, links count,
  rate bytes/sec, last-activity age (`CardActivityTracker` — `model.rs:441-498`).
- **LoRa spectrum menu** (`LoRaSpectrumMenuDetails` — `model.rs:62-73`,
  `firmware.rs:386-402`): channel busy per-mille, noise floor dBm, CCA threshold
  dBm, deferrals, contention timeouts (CCA drops), duty holds/timeouts, false
  preambles, radio recoveries.
- **BLE menu** (`firmware.rs:371-385`): recovery counters R/S/C
  (`BluetoothRecoveryMenuDetails` — `model.rs:55-60`), egress pressure events,
  supervisor peers list (`model.rs:243-274`).
- **Limits page** (`screen/limits.rs:82-169`): all storage capacities/ranges
  from `<Storage as StorageLayout>::LIMITS` (Dst, Ann, AppDst, Links, Chans,
  ChPool, MTU, LinkMDU, ChanMDU, ResBuf, ResPart, ResWin range, Retry, Fast rate,
  Receipts, PktHash, BlkHole, RevRte, PathReq, HeldAnn, Ratchet, ChanWin range).
- **Boot notices** (shown once): `IdentityReset`, `IdentityUnstable`,
  `ProfileRecovered`, `ProfileReset` (`state/mod.rs:77-95`, `firmware.rs:141-144, 311-318`).
- **Persistence state** (live): `SaveDeferred` / `SaveFailed` / `Saved`
  (`personal-hopspot/core/src/persistence.rs:3-7`; `state/mod.rs:232-257`).
- **Firmware version**: NOT on the e-ink screen, but the webUI should show it
  (USB descriptor / build const) — parity+.

### Explicitly NOT exposed (do not invent)

These do not exist as user settings on the screen boards, so the webUI must not
surface them either (would imply a feature the firmware does not have):

- **Sync word** — constant `RNODE_LORA_SYNC_WORD = 0x1424`
  (`prns-core/src/interfaces/lora/framing.rs:9`), baked into both radio drivers.
  Not a `RadioProfile` field, not editable on any board.
- **Display intensity / frontlight** — no such setting exists anywhere; the
  T-Echo frontlight is an automatic 8s hold (`t_echo/input.rs:45-54`).
- **OLED off / AP mode / radio swap / Wi-Fi station uplink** — `UiAction`
  variants exist but are unreachable on nRF52840 boards
  (`DisplayPowerControl::Unavailable`, `AccessPointState::Unsupported`).
- **BLE name / advertise toggle** — BLE identity is auto-generated
  (`bootstrap_ble_identity`, stored at `BLE_IDENTITY_FLASH_OFFSET = 0xEC000`);
  the advertised name derives from it. Only enable/disable is exposed.
- **Node name** — build-time `const` (`USB_PRODUCT`, `ANNOUNCE_APP_DATA`, etc.,
  `boards/t_echo/mod.rs:28-33`, `boards/t1000e/mod.rs:31-36`). Not runtime-settable.
- **Identity regeneration** — bootstrap-once into a `FlashVault`
  (`boards/t_echo/identity.rs:22-36`); no `UiAction` for regen. The only
  identity feedback is the boot notices.

> **Future parity+ candidates (out of scope here, raise as separate issues):**
> identity regeneration, node-name set, BLE name set, sync-word set. Each
> requires new device-side state + persistence + auth. They are NOT screen
> features today, so they are NOT webUI-parity requirements; list them as
> deliberate future extensions, not gaps.

## Architecture

### Device side (Prns firmware)

```
                    ┌─────────────────────────────────────────────┐
                    │  shared action dispatch (extracted)         │
                    │  apply_ui_action(UiAction, &Ctx)            │
                    │   → LORA_CONTROL.apply / store.save|reset   │
                    │   → *STATUS.toggle_enabled                  │
                    │   → AnnounceNow                            │
                    │   → emit UiNotice                          │
                    └───────────▲───────────────────▲────────────┘
                                │                   │
            button decode       │                   │ config lane decode
   ┌──────────────────┐         │                   │      ┌──────────────────────┐
   │  render future    │─────────┘                   │      │  config task         │
   │  (e-ink boards)   │                             │      │  (all boards, runs   │
   │  draw Card/...    │                             │      │   independent of     │
   └──────────────────┘                             │      │   render future)     │
                                                    │      └──────────▲───────────┘
                                                    │                 │
                              ┌─────────────────────┴─────────────────┴────┐
                              │  WebUSB device (existing) + config lane    │
                              │   Data lane    = Reticulum packets (unch.) │
                              │   Config lane  = UiAction req / snapshot   │
                              └──────────────────────▲─────────────────────┘
                                                     │ Chromium WebUSB (navigator.usb)
                                                     │
                              ┌──────────────────────┴─────────────────────┐
                              │  webUI (prns.dev/configure) + hopspot CLI   │
                              └────────────────────────────────────────────┘
```

Why this shape:

- **One action vocabulary.** The webUI sends the same `UiAction` variants the
  button produces. No second semantics to keep in sync.
- **One apply path.** `apply_and_persist_radio_profile` +
  `LORA_CONTROL` + `lora_profile_store` remain the only writers. The webUI
  cannot bypass validation or corrupt the store.
- **Headless works.** The config task runs as its own embassy task, not inside
  the render future, so `eink: None → pending()` no longer blocks configuration.
- **T-Echo unchanged.** The render loop still calls `apply_ui_action` after
  button decode; behavior identical. The extraction is behavior-preserving.

### Protocol extension (WebUSB config lane)

Extend `prns-core/src/interfaces/usb_auto/protocol.rs` `MessageKind` (currently
`Hello=0x01, HelloAck=0x02, Data=0x03`) with config kinds. Sketch (final opcode
allocation + wire format to be confirmed with maintainer):

```
ConfigRequest  = 0x10   # host → device: a serialized UiAction (+ request id)
ConfigResponse = 0x11   # device → host: UiNotice / apply result for a request id
Snapshot        = 0x12   # device → host: periodic or on-change state snapshot
```

- `ConfigRequest` body: `[u8 request_id][u8 action_tag][action payload...]`.
  `action_tag` maps 1:1 to a `UiAction` variant; payload is the variant's data
  (e.g. a `RadioProfile` for `SetLoRaProfile`). Reuse `RadioProfile`'s existing
  wire encoding (the same bytes `RadioProfileStore` persists) — no new
  serialization.
- `ConfigResponse` body: `[u8 request_id][u8 result_tag][...]` where
  `result_tag` is `Ok` / `ApplyFailed` / `ProfileNotSaved` / `Rejected` /
  `BadPayload`. Maps the existing `RadioProfileChangeResult` +
  `LoRaApplyOutcome` + `UiNotice`.
- `Snapshot` body: a compact serialization of the read-only surface (section
  above). Versioned (`[u16 schema_version]`) so the webUI can refuse a mismatch.
  This is the one genuinely new serializer — but it reads the same `screen/model`
  types the renderer reads.

Framing reuses the existing `RnsSerialDecoder` (HDLC-like flag/escape), so the
config lane inherits the same robustness as the `Data` lane. No new USB
endpoints — the config kinds ride the existing bulk endpoint, discriminated by
`MessageKind`. (If latency/throughput ever demands it, a separate alt-interface
endpoint can split the lanes later; not needed for config.)

### BLE transport (phase 3)

The SoftDevice + GATT-server stack already runs on nRF52840
(`nrf-softdevice` with `ble-peripheral` + `ble-gatt-server`,
`personal-hopspot/embedded/nrf52840/Cargo.toml:69`; shared
`runtime/bluetooth_auto.rs`). The existing GATT service carries the Reticulum
peer protocol. Phase 3 adds a **second GATT service** (new UUID) exposing the
same `UiAction`/snapshot semantics over a characteristic, so a phone can
configure the device over Web Bluetooth (`navigator.bluetooth`) without a cable.

**BLE config MUST be auth-gated** (see Security). The Reticulum-data GATT
service is left untouched.

### Host side

- **`hopspot configure` CLI** (extend `personal-hopspot/flasher/src/cli.rs`):
  `hopspot configure --port <...> set-lora --region EU868 --preset LongFast`,
  `... set-tx-power 14`, `... toggle-interface lora|usb|ble`, `... sleep`,
  `... announce`, `... status` (dump snapshot), `... reset-lora`. Scriptable,
  no browser needed. Reuses the flasher binary's device-list + transport code.
  This is the phase-1 host surface.
- **webUI at `prns.dev/configure`**: a browser panel (Chromium WebUSB) that
  renders the same cards/menus/LoRa-editor as the e-ink UI, plus the read-only
  panels. Reuses the phase-1 protocol. Build alongside the existing
  `prns.dev/flash` web flasher (WebUSB pattern already in `prns-js/src/browser/index.ts`
  and the `prns-wasm` browser playground).

## Security

A config endpoint writes the radio profile and re-runs apply. Today there is no
identity-regen or node-name action (section "Explicitly NOT exposed"), so the
blast radius of a config command is: change radio settings, toggle interfaces,
sleep/wake, announce. That is still a privileged surface (a malicious config
could de-tune the radio off-band, disable interfaces, or wear-level the profile
flash by spamming saves).

- **USB (WebUSB / CLI)** — physically scoped. Plugging in is possession auth.
  Adequate as-is. (WebUSB's origin model also scopes which web origin may claim
  the device — `prns.dev` only.)
- **BLE (phase 3)** — **NOT physically scoped.** Anyone in radio range could
  configure the device. A BLE config service MUST require pairing / an
  authenticated session before accepting any `ConfigRequest`. Reuse the existing
  BLE identity / pairing (do not invent a new PIN scheme without maintainer
  input). An open BLE config service is a non-starter — do not ship phase 3
  without the auth gate.
- **Rate-limit `save()`** — the profile store is wear-leveled (two-slot,
  generation-counted), but a hostile or buggy host could spam saves. The config
  task should debounce / rate-limit persist calls (the persistence journal
  already has a cooldown — `PersistenceState::Deferred` — reuse it).

## Flash transport (verified)

Resolved after the T1000-E port. Recorded for completeness (full recipe in the
agent memory; summarized here because headless config lands on top of a
flashable device):

- T1000-E stock bootloader = Nordic serial DFU. Flash with
  `adafruit-nrfutil --verbose dfu serial --package <zip> -p /dev/ttyACMx -b 115200 --singlebank --touch 1200`.
- The DFU init packet MUST match the Seeeduino build: `softdevice_req = [291]`
  (0x123 = S140 v6.x) and `device_type = 82` (0x52 = BOARD_MODEL). The
  `adafruit-nrfutil dfu genpkg` defaults (`--sd-req 0xFFFE`, `--dev-type 0xFFFF`)
  are rejected by the bootloader's init-packet check — every earlier flash
  attempt failed at the SLIP handshake until these were corrected.
- `--touch 1200` (nrfutil's own 1200-baud touch) enters DFU reliably; a manual
  python 1200-baud open/close did not.
- After flashing Prns firmware the device enumerates as a NEW identity —
  VID:PID `1209:0001`, iManufacturer "Stay Personal", iProduct "Personal
  Hopspot (T1000-E)", iSerial "PERSONAL-RNS-T1000E-HOP", single vendor-specific
  interface (class 0xFF). No `/dev/ttyACM*` — correct, Prns is WebUSB not
  CDC-ACM. Firmware liveness = USB enumerates + stable device number (no reboot
  loop). Verified 2026-08-12 on real T1000-E hardware.

The Prns `Transport` enum still lacks `NrfSerialDfu` (`prns-flash-manifest/src/catalog.rs`),
so the `prns.dev/flash` web flasher does not yet support T1000-E; first-cut
flashing is manual `adafruit-nrfutil`. That is a separate follow-up (the port
plan's part C / option C1) and is orthogonal to headless config.

## Implementation phasing

Each phase is independently mergeable. Phase 1 is the enabling slice; phases 2-3
build on it.

- **Phase 0 — done.** T1000-E port + flash + boot on hardware (PR #107 +
  verified 2026-08-12).

- **Phase 1 — device config protocol + CLI (no browser).**
  - Add `ConfigRequest`/`ConfigResponse`/`Snapshot` to
    `prns-core/src/interfaces/usb_auto/protocol.rs` (message kinds + framing;
    no new endpoint). **DONE + tested** (35 protocol tests, clippy/fmt clean
    across prns-core + embassy both boards + tokio usb + prns-wasm; repo
    `format-docs` + `personal-path-hygiene` gates pass).
  - Add a `ConfigAction` wire codec (the inner `action` payload: set/reset LoRa
    profile, toggle a named interface, sleep/wake, announce).
  - Add a config embassy task spawned in `run()` that decodes `ConfigRequest` →
    calls the existing apply primitives (`apply_and_persist_radio_profile`,
    `*STATUS.toggle_enabled`, `ui_handle.issue(AnnounceNow)`) → answers
    `ConfigResponse`, and emits `Snapshot` (on-change + heartbeat). Runs
    independent of the render future → headless boards can configure. Render
    loop untouched.
  - `hopspot configure` CLI in `personal-hopspot/flasher`: set-lora / set-tx /
    toggle-interface / sleep / wake / announce / status / reset-lora.
  - Tests: protocol round-trip (done), `ConfigAction` codec round-trip, action
    dispatch parity vs the button path's primitive calls, `RadioProfile`
    validation rejection paths, snapshot schema versioning.
  - Add a config embassy task that decodes `ConfigRequest` → `apply_ui_action`
    → `ConfigResponse`, and emits `Snapshot` (on-change + a slow heartbeat).
    Runs independent of the render future → headless boards can configure.
  - `hopspot configure` CLI in `personal-hopspot/flasher`: set-lora / set-tx /
    toggle-interface / sleep / wake / announce / status / reset-lora.
  - Tests: protocol round-trip, action dispatch parity vs button path,
    `RadioProfile` validation rejection paths, snapshot schema versioning.

- **Phase 2 — webUI over WebUSB.**
  - `prns.dev/configure` panel (Chromium WebUSB): renders the cards / LoRa
    editor / read-only panels from `Snapshot`; sends `ConfigRequest` on user
    action. Reuses phase-1 protocol.
  - Reuse the `prns-js` browser WebUSB host (`prns-js/src/browser/index.ts`)
    + the `prns.dev/flash` flasher scaffold.

- **Phase 3 — BLE config (auth-gated).**
  - New GATT config service (new UUID) alongside the Reticulum-data service,
    same `UiAction`/snapshot semantics.
  - **Auth gate required** (pairing / authenticated session) before any
    `ConfigRequest` is accepted.
  - Web Bluetooth (`navigator.bluetooth`) panel for phone-without-cable config.

## What needs maintainer buy-in

Per `CONTRIBUTING.md` ("give every concept one authoritative home"; don't expand
scope without alignment), the following shape decisions should be confirmed
before phase-1 code lands:

1. **~~`apply_ui_action` extraction~~ — resolved.** The apply primitive
   (`apply_and_persist_radio_profile`) is already a shared pub fn; the config
   task calls the existing primitives directly, so the render loop is untouched.
   No extraction, no shared-runtime blast radius. (The wire carries a
   `ConfigAction` vocabulary instead of `UiAction`, because `UiAction` is
   UI-context-dependent — see the "Why `ConfigAction`" note above. The maintainer
   may still want to weigh in on the `ConfigAction` shape.)
2. **Config message kinds + opcodes** — `0x10/0x11/0x12` allocated + tested in
   phase 1; the `Snapshot` schema-version policy + the `ConfigAction` wire
   encoding (the inner `action` payload codec) still to confirm.
3. **`Snapshot` serialization format** — CBOR vs. a fixed binary layout vs.
   reuse of an existing Prns wire codec. Affects the webUI + CLI.
4. **Whether the config lane is a new USB alt-interface endpoint or rides the
   existing bulk endpoint discriminated by `MessageKind`** (this doc assumes the
   latter; confirm).
5. **Phase-3 BLE auth model** — reuse BLE identity/pairing vs. a new scheme.
   Must be decided before any BLE config code ships.
6. **PR home** — phase 1 is larger than the T1000-E port and touches shared
   runtime. Recommend a **separate PR/branch** from `t1000e-hopspot` (#107), so
   the port lands unblocked and the config system reviews on its own merits.

## License

Same as the T1000-E port: dual MIT/Apache-2.0 (Prns house style). The webUI
frontend inherits the website's license; the device-side protocol code is
MIT/Apache-2.0. No third-party code introduced by this design.