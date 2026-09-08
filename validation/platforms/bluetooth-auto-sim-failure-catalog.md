# Bluetooth Auto simulated HIL — failure-test catalog

This catalog is the contract for a fault-injecting radio harness behind
`prns_core::interfaces::bluetooth_auto::BleBackend`. It turns field dual-role
races into named, automatable scenarios instead of one-off log dives.

It is **not** a release hardware gate. Real-radio evidence remains in:

- [`android-production-hardware.md`](android-production-hardware.md)
- [`ios-production-hardware.md`](ios-production-hardware.md)
- [`windows-ble-hardware.md`](windows-ble-hardware.md)
- board qualifications under `validation/qualifications/`

## Goal

Hand the Tokio (and later Embassy) Bluetooth Auto supervisor what looks like a
radio handle. The harness is a scriptable dual-node air model with platform
profiles and fault scripts. CI runs these scenarios without CoreBluetooth,
Android GATT, or BlueZ.

```text
ConnectionPolicy + supervisor
        │
        ▼
   BleBackend trait   ◄── Loopback / SimRadio (this catalog)
        │
        ▼
   Real backends (Mac / Android / BlueZ / WinRT / Trouble / SoftDevice)
```

Existing coverage today:

| Layer | What exists | Gap |
|---|---|---|
| Policy unit tests | `prns-core/.../policy.rs`, `handshake.rs` | Happy + selected redial cases |
| Mac decision tables | `prns-ffi/.../macos/tests.rs` | No multi-step air model |
| Loopback | `LoopbackBleBackend` in tokio `runtime.rs` | Instant usable Prns link; no faults |
| Real HIL | platform / qualification docs | Smoke and soak, not forced race scripts |

## Harness requirements

A scenario runner must provide at least:

1. **Two backends** sharing one air model (advertise / scan / ACL / GATT / CoC).
2. **Platform profiles** that change pre-dial and CoC behavior without forking policy:
   - `MacOs` — DualRole; **does** apply dial-key / `columba_connection_role`
     before dial when v5 ADV is present (option C′). CoreBluetooth uses
     manufacturer v5 dial keys because peer public MACs are unavailable;
     legacy DualRole **with manufacturer** fail-opens Dial (SoftDevice / ESP);
     no-manufacturer sightings Accept (Android UUID-only primary).
   - `Android` — DualRole; dial-key election when v5 ADV present, else MAC sort;
     empty-GATT backoff after N misses
   - `BlueZ`, `Esp32`, `Nrf52` — DualRole with address-sort (like Android)
   - `PeripheralOnly` — accept-only advertiser
   - `MacOsUnelected` — sim-only characterization of pre-fix CB (no election)
3. **Fault injectors** that fire at named steps (`OnDial`, `OnAclUp`, `OnServiceDiscovery`,
   `OnHello`, `OnWelcome`, `OnL2capOpen`, `OnSettled`, `OnWrite`).
4. **Observability hooks** — event log of `Sighting`, `Dial`, `Inbound`, `LinkReady`,
   `DialFailed`, handshake controls, settle/reject/evict, radio on/off.
5. **Deterministic clocks** — advance virtual time for `DIAL_PAUSE_MS` (15s),
   dialing backoff (16s), handshake timeout (10s), keeper window (5s).

Pass/fail criteria below are for the **pair of supervisors**, not for the
harness alone.

### Shared pass invariants (every scenario unless noted)

- At most one settled Prns link per peer `BleIdentity`.
- No unbounded dial loop: after settle-or-give-up, dial attempts to the same
  address within 60s of virtual time are bounded (scenario-specific cap).
- When a scenario expects `Bluetooth Auto Up`, both sides reach a settled
  member and exchange one application frame within the scenario deadline.
- Group mismatch never settles (`Close(Incompatible)` or no dial).

---

## Scenario index

| ID | Title | Priority | Layer | Real radio still needed? |
|---|---|---|---|---|
| BA-SIM-01 | Happy DualRole Mac↔Android | P0 | Baseline | Spot-check |
| BA-SIM-02 | Phone wins address-sort (empty GATT loop) | P0 | Role / GATT | Yes — CB attach |
| BA-SIM-03 | Mac wins address-sort | P0 | Role | Spot-check |
| BA-SIM-04 | Simultaneous dual dial storm | P0 | Keeper / redial | Spot-check |
| BA-SIM-05 | Wrong GATT role for Opens(Android) | P0 | needs_redial | No |
| BA-SIM-06 | Zombie system-connected, no Prns session | P0 | Mac admission | Yes — CB |
| BA-SIM-07 | Yield to live inbound session | P1 | Mac admission | Spot-check |
| BA-SIM-08 | Group tag mismatch | P0 | Handshake | No |
| BA-SIM-09 | Truncated ADV then full ADV | P1 | Android group cache | Spot-check |
| BA-SIM-10 | Discovery group change mid-link | P1 | Policy drop | No |
| BA-SIM-11 | GATT write timeout after settle | P1 | Recovery | Yes — timing |
| BA-SIM-12 | L2CAP open fails, GATT floor retained | P1 | Arrangement | Platform-dependent |
| BA-SIM-13 | L2CAP reader death tears link | P1 | Teardown | Spot-check |
| BA-SIM-14 | Peripheral-only peer | P1 | Election | No |
| BA-SIM-15 | Capacity: settle to MAX_PEERS, radio off | P2 | Capacity | Spot-check |
| BA-SIM-16 | Handshake flood / slack gate | P2 | Flood control | No |
| BA-SIM-17 | Weak candidate / no Prns service backoff | P1 | Dial hygiene | Spot-check |
| BA-SIM-18 | Scan silent while claimed active | P2 | Mac scan lease | Yes — CB |
| BA-SIM-19 | Inbound Hello without prior sighting | P1 | Accept path | No |
| BA-SIM-20 | Reconnect after clean close | P0 | Recovery | Spot-check |
| BA-SIM-21 | Android↔Android stays GATT-only | P2 | Arrangement table | No |
| BA-SIM-22 | Columba (non-native) peer path | P2 | Compat | Spot-check |

---

## Scenarios

### BA-SIM-01 — Happy DualRole Mac↔Android

**Intent:** Baseline that the harness and supervisors can settle the primary
production pair.

**Profiles:** A=`MacOs`, B=`Android`. Same default group (`reticulum`).

**Addresses:** Choose B MAC `<` A MAC so Android would prefer to dial if both
applied address-sort.

**Inject:** None (faithful publish + discover).

**Pass:**

1. Exactly one side ends as GATT Dialer after any redial.
2. Hello/Welcome completes with matching `group_tag`.
3. Arrangement selects Android as CoC opener; L2CAP plan runs; one data frame
   crosses.
4. Both supervisors report settled peer; deadline ≤ 30s virtual.

**Fail:** Empty-GATT redial loop; dual settled links; no frame.

**Maps to field:** Successful peer last night before the later stall.

---

### BA-SIM-02 — Phone wins address-sort (empty GATT loop)

**Intent:** Reproduce the Sep 2026 Mac↔Hopspot stuck state in CI.

**Profiles:** A=`MacOs` (no pre-dial election), B=`Android` (address-sort on).

**Addresses:** B `<` A so Android elects Dial.

**Inject (on B dial to A):**

- `OnAclUp`: ACL connects.
- `OnServiceDiscovery`: return **no Prns GATT service** (Mac peripheral never
  attaches the published service to this ACL / never emits `Inbound`).
- Mac air model: **do not** deliver `BleEvent::Inbound` / `LinkReady(Accepted)`.

**Pass (locked product behavior — option C′, both defenses):**

1. **Mac pre-dial election (when dial-keys exist):** `MacOs` applies
   `columba_connection_role` on manufacturer v5 dial keys. When the phone’s
   dial-key wins sort, Mac Accepts and Android dials. Legacy DualRole peers
   **with manufacturer** but no dial key (SoftDevice / ESP) **fail-open Dial**
   on Mac. Sightings **without manufacturer** (Android UUID-only primary)
   **Accept** — do not outbound-dial incomplete host ADV (field 15s timeout).
2. **Android empty-GATT backoff:** If a dial still connects and service
   discovery finds no Prns GATT, Android suppresses further dials to that
   address after N misses (N=3) — weak-candidate / accept-wait — instead of
   redialing every few seconds.
3. Within 60s virtual: settled peer under the elected roles, **or** (if empty
   GATT is still injected against the elected dialer) idle with
   **≤ N** dial attempts after the first miss and no unbounded client spin.

**Fail:** ≥ 10 dials in 60s all ending `no Prns service` / `DialFailed` with
Mac never logging inbound — today’s field failure. Option C′ fails closed if
either defense is missing.

**Maps to field:** Hopspot `dialer[N] no Prns service` against Mac
`F4:D4:88:6A:F6:7C`; Mac only scan/advertise.

**Real-HIL residual:** Confirm CoreBluetooth still fails to attach GATT the
same way; sim locks the *policy* response.

---

### BA-SIM-03 — Mac wins address-sort

**Intent:** Opposite sort order; Mac should dial, Android accept.

**Profiles:** A=`MacOs`, B=`Android`. Addresses A `<` B.

**Inject:** None.

**Pass:** Mac initiates (wins sort). Handshake may `needs_redial` so Android
becomes the Opens(Android) CoC dialer; both sides settle; frame path available.

**Fail:** No settle; empty-GATT spin; dual settled members for one identity.

---

### BA-SIM-04 — Simultaneous dual dial storm

**Intent:** Both sides dial before either settles.

**Profiles:** Both `Android`-like DualRole **without** waiting for election
(force both to dial), or Mac+Android with staggered sightings so both call
`dial()` in the same virtual tick.

**Inject:** Both `OnDial` succeed into full Prns GATT (usable links both ways).

**Pass:** Keeper duel within 5s leaves **one** settled identity; loser evicted
or rejected; dial pause respected (`DIAL_PAUSE_MS`); one frame on the keeper.

**Fail:** Two settled members for one peer; livelock redials past pause.

---

### BA-SIM-05 — Wrong GATT role for Opens(Android)

**Intent:** `needs_redial` for Mac↔Android when the non-opener is Dialer or the
opener is Listener.

**Profiles:** A=`MacOs`, B=`Android`.

**Inject:** Force first settle with Mac as Dialer (wrong for Opens(Android)).

**Pass:** Policy rejects; Android (opener) redials as Dialer within pause rules;
second settle has Android Dialer / Mac Listener; CoC opens from Android.

**Fail:** Settled wrong-role link kept; or neither side redials.

**Note:** Already partially unit-tested in policy; this scenario runs it through
the full supervisor + backend event loop.

---

### BA-SIM-06 — Zombie system-connected, no Prns session

**Intent:** Mac dial admission `CancelStaleSystemConnection` vs infinite yield.

**Profiles:** A=`MacOs`, B=`Android` (or `Esp32`).

**Inject:**

- Air reports peer ACL **system-connected**.
- No Prns central/peripheral session registered on A.
- A attempts dial after sighting.

**Pass:** Admission cancels stale connection and attaches a new central session
(or equivalent `CancelStale` path); settle succeeds before
`STALE_CANCELLATION_FALLBACK_TTL` budget is exhausted repeatedly without
progress.

**Fail:** Permanent `YieldToSystemConnection` with no cancel; Bluetooth Auto
Down forever while sightings continue.

**Maps to field:** Earlier Mac MeshTower “yielding dial … already connected
system-wide” zombie class.

**Real-HIL residual:** CB Connected semantics.

---

### BA-SIM-07 — Yield to live inbound session

**Intent:** Do not dual-role dial when inbound Prns session already owns the peer.

**Profiles:** A=`MacOs`, B=`Android`.

**Inject:** Complete inbound settle on A; then inject sighting that would dial B.

**Pass:** Dial admission yields to inbound; no second link; settled link stays up.

**Fail:** Outbound dial tears or duplicates the live session.

---

### BA-SIM-08 — Group tag mismatch

**Intent:** Handshake is authoritative for discovery groups.

**Profiles:** A=`MacOs` group `reticulum`; B=`Android` group `other`.

**Inject:** Sightings allowed (ADV may even look compatible); Hello/Welcome tags
differ.

**Pass:** `Close(Incompatible)` or reject; no settled peer; no data plane.

**Fail:** Settled across groups.

---

### BA-SIM-09 — Truncated ADV then full ADV

**Intent:** Android must not permanently blacklist a same-group peer when the
first sighting lacks manufacturer data.

**Profiles:** A=`MacOs`, B=`Android`.

**Inject:**

1. Sighting with **no** manufacturer payload (truncated).
2. Later sighting with full v4 group tag matching B.

**Pass:** B eventually dials or accepts; inbound path does not reject solely on
“unknown” cache; settle succeeds.

**Fail:** `peerDiscoveryAllowed=false` stuck; tear-down of valid peer; never dial.

**Maps to:** `BleLink.kt` comments on truncated ads / `inboundDiscoveryPermitted`.

---

### BA-SIM-10 — Discovery group change mid-link

**Intent:** Group change drops peers and does not linger across groups.

**Profiles:** Both DualRole, same group; settle; then A switches group tag.

**Inject:** `drop_all_links` / group wake as Android backend does.

**Pass:** Settled link closes; radio may restart; no frames to old peer; new
group only peers with matching tag.

**Fail:** Old peer remains settled after group change.

---

### BA-SIM-11 — GATT write timeout after settle

**Intent:** Settled link death is observable and recoverable.

**Profiles:** A=`MacOs`, B=`Esp32` (GATT-floor pair) or Android on GATT fallback.

**Inject:** After settle, `OnWrite` times out (`GattWriteTimeout`); close link.

**Pass:** Peer removed; backoff then rediscovery; second settle within deadline
unless zombie inject from BA-SIM-06 is also active.

**Fail:** Supervisor thinks peer still up; or rediscovery wedged on stale ACL.

---

### BA-SIM-12 — L2CAP open fails, GATT floor retained

**Intent:** Platform FailurePolicy for central retaining GATT when CoC fails.

**Profiles:** A=`MacOs` (central retains floor), B=`Android` (opener).

**Inject:** `OnL2capOpen` fails after Welcome.

**Pass:** Data plane remains on GATT floor (Mac-as-central policy); or documented
inbound EndLink behavior if roles reversed — scenario must assert the
**profile-specific** FailurePolicy explicitly.

**Fail:** Silent half-open “settled” with no send path.

---

### BA-SIM-13 — L2CAP reader death tears link

**Intent:** Inbound CoC read error tears the Prns session cleanly.

**Profiles:** A=`MacOs`, B=`Android`; L2CAP up.

**Inject:** `OnL2capRead` error / EOF after settle.

**Pass:** `receive closed`; member gone; radio reconcile; rediscovery possible.

**Fail:** Zombie settled member with dead CoC.

**Maps to field:** Mac `L2CAP reader exited` then Android identity closed.

---

### BA-SIM-14 — Peripheral-only peer

**Intent:** DualRole always dials PeripheralOnly; two PeripheralOnly never dial.

**Profiles:** A=`Android` DualRole, B=`PeripheralOnly`.

**Pass:** A dials; B accepts; settle.

**Variant:** Both `PeripheralOnly` → no dial, no settle (Unavailable).

---

### BA-SIM-15 — Capacity MAX_PEERS

**Intent:** Advertising/scanning stop at capacity; resume when a slot frees.

**Profiles:** One supervisor with `MAX_PEERS` (use test const 2 or 3), N peers.

**Pass:** At capacity, `set_advertising/scanning(Off)`; closing one peer turns
radio On and allows a new peer.

**Fail:** Dial attempts while at capacity without flood control; radio stuck Off
after free slot.

---

### BA-SIM-16 — Handshake flood / slack gate

**Intent:** Inbound handshake count limited by `MAX_PEERS + HANDSHAKE_SLACK`.

**Inject:** Many concurrent `Accepted` handshakes.

**Pass:** Excess rejected/not started; existing settled unaffected.

**Fail:** Unbounded handshaking slots / stall.

---

### BA-SIM-17 — Weak candidate / no Prns service backoff

**Intent:** Repeated service-miss against a sighted address suppresses dials
(Mac weak-candidate ~5 min; Android should not spin every few seconds forever).

**Profiles:** A=`MacOs` or B=`Android` as dialer.

**Inject:** Persistent empty GATT on target (like BA-SIM-02) for many sightings.

**Pass:** After threshold, dial rate drops to suppressed backoff; no tight loop.

**Fail:** Continuous dial every ~5–10s indefinitely (field Hopspot loop).

---

### BA-SIM-18 — Scan silent while claimed active

**Intent:** Mac scan-lease restart when no callbacks for `RADIO_LIVENESS_INTERVAL`.

**Profiles:** A=`MacOs`.

**Inject:** Scanning On but air delivers no callbacks for lease period; then peer
advertises.

**Pass:** Harness/backend restarts scan; subsequent sighting delivered.

**Fail:** Permanent silent scan; peer never sighted.

**Real-HIL residual:** CoreBluetooth scan death.

---

### BA-SIM-19 — Inbound Hello without prior sighting

**Intent:** Listener path works when peer dials before local sighting cache.

**Profiles:** A=`MacOs`, B=`Android`; only B sights and dials; A gets Inbound only.

**Pass:** A settles as Listener; no requirement that A dialed.

**Fail:** Drop inbound because address unknown.

---

### BA-SIM-20 — Reconnect after clean close

**Intent:** After mutual close, pair re-peers without manual restart.

**Profiles:** A=`MacOs`, B=`Android`; settle; inject clean close both sides;
continue ADV/scan.

**Pass:** Second settle within deadline; single link.

**Fail:** One side stuck in backoff; or BA-SIM-02 loop on second attempt.

---

### BA-SIM-21 — Android↔Android stays GATT-only

**Intent:** Arrangement table `GattOnly` for Android↔Android.

**Profiles:** Both `Android`.

**Pass:** Settle without CoC upgrade; data on GATT; no L2CAP open attempts.

**Fail:** Spurious CoC race between two Androids.

---

### BA-SIM-22 — Columba compatibility path

**Intent:** Non-native Columba GATT peer still handshakes on the supported path.

**Profiles:** A=`Android` or `MacOs`, B=`Columba` (harness peer_protocol).

**Pass:** Documented Columba identity/control path completes or cleanly refuses
per current compat rules (assert explicitly in the test).

**Fail:** Native supervisor panic or wedged dial on Columba chars.

---

## Implementation phases

### Phase 0 — Document lock (this file)

- **Done:** BA-SIM-02 product invariant is option C′ (both defenses; legacy
  DualRole fail-open Dial on Mac — revised 2026-09-04 after Mac↔HV4 regression).
- Keep this catalog as the scenario ID namespace.

### Phase 1 — Sim radio MVP (P0 scenarios)

- **Done (2026-09-03):** `SimBleBackend` air model + platform profiles in
  `prns-interfaces/impls/tokio/src/bluetooth_auto/ba_sim.rs`.
- Scenarios automated: BA-SIM-01..22 (`ba_sim_` filter; Phase 1 P0 plus
  Phase 2/3 admission, recovery, capacity, and compat).
- Option C′ in sim: MacOs applies `columba_connection_role` at sighting edge
  (address stand-in for dial-key); Android **and** MacOs dialers suppress after
  `EMPTY_GATT_MISS_LIMIT` (3) empty-GATT misses.
- Production Android: empty-GATT miss backoff in `BleLink.kt` (N=3, 5-minute
  suppress). Mac CoreBluetooth elects on **dial key** (manufacturer v5 = first
  6 bytes of `BleIdentity`) when present; **legacy DualRole with manufacturer
  but no dial key fail-opens Dial** (SoftDevice / ESP / HV4). Sightings **without
  manufacturer** (Android UUID-only primary before scan response) **Accept** —
  fail-open Dial there raced the phone inbound and burned a 15s dial timeout on
  a concurrent HV4 dial (2026-09-04). `MacOsUnelected` remains in `ba_sim` as a
  characterization of the pre-fix unelected race.

```console
cargo test --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml \
  --features bluetooth-auto ba_sim_
```

### Phase 2 — Admission and recovery (remaining P1)

- **Done (2026-09-03):** BA-SIM-06, 07, 09, 10, 11, 12, 13, 14, 17, 19 in
  `ba_sim.rs` (admission/lease decision tables + air-model scenarios).
- Mac dial-admission / scan-lease mirrored in-sim as pure functions matching
  `prns_ffi::bluetooth_auto::macos::backend` enums.

### Phase 3 — Capacity / lease / compat (P2)

- **Done (2026-09-03):** BA-SIM-15, 16, 18, 21, 22 automated under the same
  `ba_sim_` filter. Embassy supervisor hook remains optional follow-up.

### Phase 4 — Thin real-HIL mirrors

For each P0 scenario marked “Real radio still needed,” add a short subsection
to the relevant platform gate that cites the BA-SIM-ID and records one manual
or scripted device run per release train. Sim remains the always-on gate;
hardware confirms the model.

## Evidence shape (sim CI)

Each scenario test should print or assert structured names:

```text
BA-SIM-02 PASS profiles=MacOs,Android dials_after_miss=2 settled=false idle=true
```

Artifact optional under `validation-artifacts/` when wired into
`validation/run.py`; until then, owning `cargo test` filters are enough:

```console
cargo test --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml ba_sim_
```

## Product decision (BA-SIM-02)

| Option | Behavior under empty GATT + phone-wins sort |
|---|---|
| A | Mac applies `columba_connection_role` like Android (phone Accept-only) |
| B | Android suppresses dial after N service misses and waits as peripheral |
| C | Both A and B (first lock 2026-09-03) |
| C′ | Both, with dial-key election when v5 present; **legacy DualRole fail-open Dial** on Mac |

**Decision: C′** — revised 2026-09-04 (was C).

Rationale: dial-key election on Mac removes the asymmetric sole-dialer hole for
Mac↔Android; Android empty-GATT backoff still contains the failure if election
is wrong, ADV is stale, or a DualRole peer omits the filter. Accept-only for
legacy DualRole (no dial key) was part of the first C implementation and
regressed Mac↔SoftDevice/ESP (mutual Accept vs Mac RPA). C′ keeps both
defenses without that Accept-only rule. BA-SIM-02 asserts both defenses in sim;
`MacOsUnelected` keeps the pre-fix unelected race visible in CI.

## Related code

- Trait: `prns-core/src/interfaces/bluetooth_auto/backend.rs`
- Policy: `prns-core/src/interfaces/bluetooth_auto/policy.rs`
- Handshake / arrangement / needs_redial: `.../handshake.rs`
- Election: `.../advertisement.rs` (`columba_connection_role`)
- Loopback seed: `prns-interfaces/impls/tokio/src/bluetooth_auto/runtime.rs`
- Mac admission: `prns-ffi/src/bluetooth_auto/macos/backend.rs`
- Android dial filter: `personal-hopspot/mobile/android/.../BleLink.kt`
