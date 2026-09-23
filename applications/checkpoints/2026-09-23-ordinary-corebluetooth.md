# Ordinary CoreBluetooth feasibility

Branch: `prns-app`. The user approved testing board-free, automatic BLE networking
on September 23, accepting loss of ASK's additional force-quit relaunch behavior.
This is the transport-feasibility slice of the [phone demo](../docs/phone-node-demo.md),
not completion of the planned messaging UX or network inspection.

## Implementation

- Removed AccessorySetupKit adoption metadata, framework linkage, accessory
  roster, picker and per-accessory startup gate.
- iOS uses the existing public dual-role AutoBLE preparation API with distinct,
  variant-specific central/peripheral restoration identifiers. The original
  central identifier remains stable. Both Bluetooth background modes are present.
- Retained one process-owned Rust node and prepared manager handoff, early native
  restoration, existing-identity checks, bounded protected-data recovery,
  concurrent-start coalescing and generation-safe Stop/reset cancellation.
- Ordinary app-level Bluetooth permission is requested by the actual manager,
  not a second permission-probe manager. Automatic restoration only runs with
  authorization already granted; it cannot request permission in the background.
- The phone node and saved data do not depend on an accessory or granted Bluetooth
  permission. Denial/restriction is shown independently from local node state.
- If startup loses foreground eligibility before an undecided permission prompt,
  the iOS card offers an explicit retry. Repeated presses are coalesced; permission
  changes do not automatically restart a stopped node or reset saved data.
- Regenerated TypeScript, Swift and Kotlin bindings for the dual-role lifecycle
  API. Android transport behavior is unchanged.

Apple confirms that ASK changes the app-wide authorization model and prevents
[peripheral-manager authorization](https://developer.apple.com/forums/thread/797137).
Ordinary CoreBluetooth still supports background Bluetooth events and restoration;
ASK adds exceptions for [force-quit and Control Center Bluetooth toggling](https://developer.apple.com/forums/thread/806013).
Neither path guarantees continuously running background code or message delivery.
User-visible incoming-message notifications remain a separate unimplemented feature.

## Qualification boundaries

New device evidence is required; earlier ASK tests do not qualify this migration.
In particular, restoring a peripheral's GATT service does not reconstruct its Prns
peer session. A surviving central needs a fresh handshake/reconnection. Central-
role restoration and peripheral-role restoration need separate evidence.

The intended physical sequence is:

1. Retained-data install on MetalbeardMobile; observe ordinary Bluetooth consent
   and a running local node without any board or per-peer picker.
2. Automatic discovery/connection to Galaxy S9+, reciprocal LXMF announces and
   proof-backed short messages. Inspect BLE-only routes/counters; USB and Metro
   must not carry the messages.
3. Locked/same-process incoming receipt, first request after wake, then a separately
   recorded controlled process-restoration trial. Do not call a developer kill
   natural suspension or OS memory-pressure evidence.
4. Permission denial/recovery and cold retention. Force-quit recovery is not an
   acceptance requirement. Two-iPhone interoperability remains unqualified.

No board firmware, board settings or app identities have been reset for this
work. No PR or branch push is included.

## Automated/build evidence

- 189 native unit tests and four integration tests passed.
- 325 app tests and 59 SDK tests passed, including offline authorization,
  stale status events and explicit foreground-start retry.
- App/SDK typechecks, formatting/lint, route/version/config checks, generated API
  checks, binding/tool tests and compatibility checks passed.
- Swift authorization/start dispatch, protected-data recovery, restoration
  diagnostics/release-symbol checks passed. Nine metadata cases verify both
  Bluetooth background modes/identifiers and reject ASK adoption keys.
- Rust Release iOS framework and signed Xcode 27 Release app built successfully.
  Deep signature verification passed. Compiled metadata has iOS 18 minimum,
  both background modes/identifiers, one Expo scene, no ASK keys/framework linkage
  and no development TCP fixture. The embedded framework exports the new dual-role
  lifecycle API. Embedded JavaScript includes the retry correction; Metro is not
  required.

The build reused
`/Volumes/wavlink/dev/prns-ios-metalbeard-20260922.FotaWv/Build/Products/Release-iphoneos/prnsdev.app`.
It is signed for `rs.reticulum.prns.dev`, team `MWDZ9M6W55`, with existing profile
`08e00f9b-93e1-4beb-9382-e8ac7956cfc9` including MetalbeardMobile. No developer-portal
changes were requested. Xcode's device-specific destination timed out; building
for generic iOS succeeded with the shell's macOS `LIBRARY_PATH` removed.

SHA256 values:

- App executable: `864f658f32bd00483637ff4e69c62d5948e5e37357239ab4010423faede24a82`
- Embedded JavaScript: `ea698a841b2ed5ba13897d76c60bbff2ffbf6e9f66f098cf2e962f228eeec330`
- Embedded Rust framework: `2b34696380e90b076fc9a11bdad63602ced98882ed5b6a165b1997e8e833accd`

Local build logs and bundle-verification receipt are in
`scratch/prns-app/2026-09-23/ordinary-corebluetooth/`.

## Initial device availability

Galaxy S9+ (`444a443651523098`) was reachable over USB, Android 10, with its existing
Release app running. No Android installation was needed for the first peer test.
MetalbeardMobile (`00008120-001E25C63E60C01E`, iOS 27) initially appeared prepared
over the network and locked, but was not present in the USB inventory. Subsequent
CoreDevice connections and iPhone Mirroring timed out. The user was asked to
connect it by USB and unlock it.

At that initial stop, neither phone had received a new installation. The
following continuation replaces that availability blocker with new-build evidence;
the earlier ASK installation does not qualify this path.

## Physical continuation: direct, unpaired BLE messaging

On September 23, the verified Release above, source `f606a3f5c`, was installed on
MetalbeardMobile over retained data and launched as PID 16324. Apple tools used
their local-network tunnel despite the cable; libimobiledevice could not attach
its logger. Installation and launch receipts are in the local evidence directory.
The user confirmed normal startup, accepted the ordinary Bluetooth prompt and
locked the phone for Mirroring. Saved pairing and the existing 13-message
conversation remained visible. No accessory chooser, phone-to-phone OS pairing,
RemoteControl pairing, identity reset or board operation was used.

Galaxy retained its September 22 Release, source `cabb9fad1`, PID 11925, APK SHA256
`3614ee7fee0419df0ceef7923eaa576619d3cd4b25f4fca96b4739b389dec49a`.
Both apps use embedded JavaScript; no Metro or TCP test fixture carried messages.

### Discovery and transport evidence

- iPhone displayed Running, Native, Bluetooth-only capabilities and Connected
  AutomaticBluetoothLe. Its initial packet counters and route count were zero.
- Each phone's existing Share messaging address action caused the other phone
  to show a newly heard `prns` peer. No address was copied or manually entered.
- iPhone messaging destination: `ade5b79ac66f17d6b1b9685fe6ae57ae`.
  Galaxy messaging destination: `549dfdc562189fd3b10a8523bf8ed8e7`.
- iPhone's route to the Galaxy was one hop, Direct, through Bluetooth interface
  `0cde82ef07008c6d`. Its verified identity association matched the Galaxy's own
  primary identity, `5235c813b0219ff915bc86f8725b51f4`.
- Galaxy's GATT system state showed one live peer, Android listener connection 4,
  and no outgoing GATT clients throughout the exchanges. Its peer address was
  absent from the bonded-device list. Initial role arbitration settled into
  iPhone central / Android peripheral at 07:59:27 EDT, with no subsequent
  disconnection or role change through the final 08:12:55 snapshot.
- Android DATA frames correlate with all four exchanges below. At an iPhone
  08:12 snapshot, host and sole Bluetooth-interface counters both reported
  2,382 received / 2,350 sent bytes. These isolated-test observations establish
  the BLE path; the app does not yet expose historical per-message paths.

### Message results

Times are EDT on September 23, 2026. Both receiving UIs were inspected. Delivered
is the existing proof-backed result, not a read receipt.

| Trial | Message | Sender result |
| --- | --- | --- |
| iPhone → Galaxy, foreground | `ble0808` | Sent 08:08:02; delivered 08:08:03, 211 ms |
| Galaxy → iPhone, foreground | `BLE Galaxy to iPhone 0808` | Delivered 08:08:38, 84 ms |
| Galaxy → off-screen iPhone | `BLE iPhone background 0810` | Delivered 08:10:22, 42 ms, before reopening Mirroring |
| First iPhone send after resume | `resume0811` | Delivered 08:11:28, 181 ms |

For the off-screen trial, the iPhone app was sent Home and Mirroring was quit
around 08:09; Mirroring was confirmed absent at 08:09:36. The Galaxy submitted
the message at 08:10:22. After reconnecting Mirroring, the iPhone showed that
exact stored message with its 08:10:22 receipt time. PID 16324 remained unchanged
before and after this trial. The connection carried periodic traffic throughout.

This establishes a bounded, same-process background receipt on an existing link,
not prolonged idle, natural suspension, independently verified continuous lock,
or OS termination/restoration. No iOS lifecycle log was captured; the local
`device-lifecycle.log` contains only the logger's waiting message. Android's
nonclearing capture is `android-ble-live.log`, with focused system snapshots
`android-gatt-after-background.txt` and `android-gatt-final.txt`. Both temporary
host loggers were stopped; phone runtimes were left running.

### Remaining qualification and product work

The ordinary CoreBluetooth choice now has direct mixed-phone transport evidence.
Permission denial/recovery, explicit interface controls, cold retention on this
binary, out-of-range recovery, quiet/long idle and controlled process restoration
remain separate trials. Two-iPhone operation and Android background receipt were
not tested here. Incoming notifications are still unimplemented. Board-control
compatibility needs its own new-build check.

The existing UI can perform these tests but does not complete the demo: announcing
is hidden, both names are hardcoded `prns`, and discovered peers appear as empty
conversations. Continue with the planned name/announce/discovered-contact journey,
truthful Bluetooth status and network inspection; do not rewrite the working
messaging transport. No push or PR update was performed.

The subsequent [ASK cleanup](2026-09-23-ask-cleanup.md) removes unused central-only
core APIs, refreshes diagnostics and records the separate upstream PR changes.
It does not extend this binary's physical qualification to a new build.
