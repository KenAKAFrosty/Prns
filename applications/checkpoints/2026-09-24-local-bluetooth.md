# Local Bluetooth controls and physical connection status

The Connections slice now has retained-data acceptance on iOS and Android.
App Bluetooth-off persisted across cold launches on both platforms. Disabling
Bluetooth leaves the node running within its existing process; enabling it
reconnects. Both final installed binaries reported one physical peer after the
larger-text layout fixes. The checks repeated after those fixes are recorded
separately below. This is a bounded foreground/status checkpoint, not completion
of the
[two-phone messaging demo](../docs/phone-node-demo.md).

## Behavior and ownership

**Nodes → This phone → Connections** and **More → Connections** show the local
Bluetooth state, its stored enable/disable setting and connected physical peers.
Ready with zero peers is distinct from Connected. Counts come from Bluetooth
fleet members; routes, Reticulum Links, contacts and saved board pairings are
not counted as physical connections. Peer IDs identify physical interfaces,
not authenticated messaging identities. Traffic and optional transport/RSSI
details use only facts the native interface publishes.

The app owns the preference, storage outcome, status wording and UI. The existing
native database owner commits the preference before applying it to the existing
AutoBLE supervisor. Accepted changes continue if the JavaScript waiter is
cancelled. Startup reads that same preference, including when the node was
previously stopped. Toggling Bluetooth does not restart the node or clear saved
identity, contacts, pairings or mail.

Shared Rust owns radio observation and supervisor enable/disable mechanics.
Apple status observes the existing CoreBluetooth managers; no second manager is
created for UI inspection. Android combines supervisor facts with its existing
platform permission, radio and required location-service checks. Unknown or
failed access checks do not establish readiness. A native-only inspection result
retains raw physical members from the capture used to assemble the logical
HostSnapshot. The foreign HostSnapshot schema is unchanged, and the app still
borrows the general SDK's host through the single aggregate native image.

The UI remains Turning off until the supervisor acknowledges the current disable
request and the captured physical peer rows are gone. This acknowledgment does
not mean the operating system radio is powered off. On Android, it means the
Rust supervisor/backend completed its shutdown requests; the backend updates
bridge flags and wakes Kotlin workers. It does not acknowledge that those OS
workers have joined, and must not replace the foreground service's existing
shutdown/join checks for resource release or listener replacement.

Source: [app projection](../prns/native-composition/src/bluetooth.rs),
[preference and lifecycle admission](../prns/native-composition/src/lifecycle.rs),
[shared inspection](../../prns-host/impls/native/src/lib.rs), and
[shared Bluetooth supervisor](../../prns-interfaces/impls/tokio/src/bluetooth_auto/runtime.rs).

## Installed artifacts and retained evidence

The initial control and retained-data acceptance used these artifacts:

| Artifact | SHA-256 |
| --- | --- |
| Android Release APK | `c83d678fbcac040488d15a7c3fcd04d97314545252ec65cd477ff87ee860ac5d` |
| iOS executable | `ee63a909d953acd7a5c83d641fa7c53308c9df9174652691015e57cd0b207a13` |
| iOS bundled JavaScript | `bc13c843a1a3cba72c75a32084a29a3021393a11e47fdf40578663aa6347eca7` |
| iOS aggregate PRNS framework | `ed6adadfd58e2b38454a1fc40d2b1f942cc74c93bf7236118e69740bea732798` |

The subsequent larger-text fixes were packaged in these final artifacts:

| Artifact | SHA-256 |
| --- | --- |
| Android Release APK | `e05960f79e3a4f7f7202d7d6ce4c4386519e6364dce99fa9ebff617d82df9fea` |
| iOS executable | `6f34b2bdb0cb5db9a7270271d9a202f74204de7061bfd1069905ceef6335b9ca` |
| iOS bundled JavaScript | `a8d4f73d0dd8a2978a7268b1970ee6223dd26e74199d8f6cc9905b0c6f93ecb5` |
| iOS aggregate PRNS framework | `24dc2fc05d3fc917fe90f8682b63d2aa0264e7134cfc057d2ac0ad3bc291d6b1` |

Local evidence is retained under
`/Volumes/wavlink/dev/prns-bluetooth-20260924/`: `build-source.json` records build
input hashes, `android/summary.json` records Android observations and artifact
identity, and `ios/summary.json` records the iOS observations. The iOS
`disabled/comparison.json` and `restored/comparison.json` record initial
retained-data comparisons. The initial iOS hashes are preserved in
`ios/artifact-before-layout-fix.json`; final artifact identities are recorded in
`android/layout-artifact.json` and `ios/artifact.json`.

`final-build-source.json` records the initial provenance audit of 54 task source
files. `layout-build-source.json` records the final 57-file audit: the production
UI sources match the build checkout; differences are limited to two evolving
documents, two test files and Rust module-order formatting. Android's packaged
bundle has matching source-map contents, and all 22 native libraries are
byte-identical to the initial acceptance APK. An iOS source map was not retained,
and the iOS native artifact hashes changed; no byte-identical-native claim is
made for iOS. These audits establish artifact identity and captured build inputs,
not reproducible builds or device acceptance. This checkpoint and its linked
status updates continued after the audits.

Initial device acceptance:

| Check | Android | iOS |
| --- | --- | --- |
| Retained-data installation | Passed; update installation without clearing data or resetting permissions | Passed; installation retained the existing app data |
| App Bluetooth off while node keeps running | Passed | Passed |
| Off preference survives cold launch | Passed | Passed |
| Existing mailbox remains readable while app Bluetooth is off | Passed | Passed |
| Enable and reconnect | One physical peer reported | One physical peer reported |
| OS radio off and on recovery | Passed; Off was shown while the node remained running, then one peer returned | Not tested in this slice |
| Permission-denial recovery | Not tested in this slice | Not tested in this slice |
| Larger text | 1.5x and 2x passed; controls remained reachable | Exposed heading/tab layout defects; fixed and retested on the final layout build below |

Android retained the displayed primary/controller identities, one saved pairing,
zero contacts and two conversations containing eight and three messages. Its
Release app was not debuggable, so this is UI evidence rather than direct
database inspection. The iOS database comparisons preserved both identities,
one pairing, two contacts and all 21 messages through disabled and restored
captures. No new messages were sent on either platform during this slice.

Android's original font scale of 1.1 was restored, and its system and app Bluetooth
settings were left enabled. Bluetooth-off readability here does not mean all
network access was disabled: Wi-Fi and cellular settings were preserved.

Initial iOS larger-text checks exposed vertically clipped tabs at maximum
accessibility text and a Connections heading squeezed to one character per line
at a smaller enlarged setting. The final layout uses content-sized tab labels
and stacked card headings/actions at large text. On the final iOS artifact, the
system's maximum accessibility text setting showed unclipped tabs, allowed
navigation through More to Connections, and displayed the Bluetooth heading
readably. Scrolling reached the Disable control, which was pressed. Long words
and tab labels wrap onto multiple lines at this size. No numeric font scale was
measured, and this is not a separately measured iOS 1.5x/2x result.

The final iOS artifact preserved the disabled setting across a cold launch and
showed Bluetooth Off while the node was Running. Enabling Bluetooth from
Connections reconnected one physical peer. The comparisons in
`ios/layout-final/comparison.json` and `ios/final-restored/comparison.json` again
preserved both identities, one pairing, two contacts and all 21 messages. The
original iOS text settings were visually restored, including Larger
Accessibility Sizes off and the original slider position; the compact layout
was readable again. System and app Bluetooth were left enabled.

The final Android APK was installed over the existing app, then cold-launched at
2x text. The Nodes and Connections headings remained readable, all tabs and the
Disable/Refresh controls were reachable, and Refresh was exercised. The original
1.1 font scale was restored; the default layout fit and the running node reported
one physical peer. UI checks again showed the same identities, one pairing, zero
contacts and conversations with eight and three messages. App-off/cold-launch
and OS-radio recovery were not repeated after this layout-only repackage; those
checks above apply to the initial APK with byte-identical native libraries.

## Local automated checks

- The initial full app suite passed 353 tests and the platform suite passed 64
  tests. Four additional Connections regressions passed in a focused 28-test
  run. After the final larger-text layout corrections, the full app suite
  passed all 357 tests (`app-layout-v2-tests.log`).
- The app Rust library passed 192 tests. After the final shutdown-acknowledgment
  projection, all nine focused Bluetooth tests passed with the Apple feature.
  Coverage includes stored-off restart behavior, cancellation after admission,
  retained mail, corrupt preference handling, physical-member counting,
  radio/failure priority and pending disable with no peers.
- The native inspection attach/detach/stopped test and the generated foreign
  snapshot integration passed. Shared Bluetooth passed 21 tests and strict
  iOS-target Clippy; app Clippy with Apple/host-test features also passed.
- Final generated API drift, the app/SDK dependency boundary and formatting of
  all 23 changed Rust files passed. The final app contract fingerprint is
  `prns-app-native/local-node-1/af0c730af9c064bd`.

A separate maintenance correction updates third-party notice graph assertions
for the extracted native/Expo SDK. Its 13 tests passed; it changes no Bluetooth
behavior and is not physical acceptance evidence.

## Remaining qualification and next work

iOS OS-radio recovery and permission-denial recovery on both platforms remain
untested here. Neither background/locked operation, natural suspension/restoration,
long idle nor USB-unplugged operation was accepted on these binaries. Existing
September 23 message-delivery results
remain evidence for those earlier binaries; this slice did not repeat delivery.

Persisted messaging names and the Saved/Discovered contact journey remain the
next app feature slice. Keep further recovery and background trials separately
recorded, and keep general transport mechanisms below app product policy.
