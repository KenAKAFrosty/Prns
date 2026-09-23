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

## Device status: not installed or physically qualified

Galaxy S9+ (`444a443651523098`) was reachable over USB, Android 10, with its existing
Release app running. No Android installation was needed for the first peer test.
MetalbeardMobile (`00008120-001E25C63E60C01E`, iOS 27) initially appeared prepared
over the network and locked, but was not present in the USB inventory. Subsequent
CoreDevice connections and iPhone Mirroring timed out. The user was asked to
connect it by USB and unlock it.

No new app has been installed on either phone in this checkpoint. No consent,
automatic discovery, message exchange, retained-data install, locked receipt or
OS restoration result is claimed. Install the verified standalone build when the
iPhone is available, then run the physical sequence above. The old installed ASK
build does not exercise this new code.
