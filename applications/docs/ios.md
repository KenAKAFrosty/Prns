# iOS development

The iOS app uses the generated bindings and shared Rust image described in the
[binding guide](../prns/native-composition/bindings/README.md). Start with
[workspace setup](../README.md#setup). Historical observations and current-build
trials have different scopes. Earlier frameworks have bounded foreground and
restoration-requested relaunch evidence, not full lifecycle qualification; see
[current validation and limits](validation.md).

The app uses Expo's single-scene lifecycle, required when building with Xcode 27
for iOS 27. On SDK 57 this is enabled through `expo-build-properties` with
`ios.enableSceneSupport: true`; Expo 57.0.23 contains the scene runtime. Clean
prebuild declares `EXExpoAppSceneDelegate` and makes AppDelegate provide the React
factory without starting the window itself. Scene creation owns the UI only,
not the native node. See [Expo's SDK 57 guidance](https://github.com/expo/fyi/blob/main/ios-scene-lifecycle.md#staying-on-sdk-57-with-xcode-27).

## iOS native lifetime and Bluetooth restoration

The implemented direct-LXMF slice retains one native node for the iOS process,
including React provider remounts and shell routes that do not render network
state.
Inbound Link DATA is asynchronous and must continue to reach the aggregate's
sole Rust event consumer before the user opens Inbox; moving between shell
routes or unmounting the Expo module therefore does not stop and restart the
node. Explicit stop and development reset still use the bounded native
shutdown path.

An earlier signed iPhone installation passed a physical UI
reload with native uptime continuity and a fresh authenticated peer check.
This does not qualify natural suspension, background wake, or process
restoration; the full physical matrix remains pending. See the
[validation summary](validation.md) for later restoration observations
and their limits.

Native lifetime is process-owned rather than scoped to a consuming screen.
Low-frequency development snapshot refresh runs only on `/nodes` and `/inbox`
routes and their descendants. Other shell routes keep the node and event owner
alive without periodic snapshot reads.

The app uses ordinary CoreBluetooth authorization, not AccessorySetupKit or a
per-accessory chooser. Explicit foreground Start creates the existing Rust-owned
Bluetooth managers and can trigger the normal app-level permission prompt. No
external board, OS bond or RemoteControl grant is needed to run the phone's node.
Permission denial and a powered-off radio do not prevent the local node from
starting or hide saved Inbox/Contacts data; Bluetooth remains unavailable until
the OS permits it. The native authorization status is refreshed on foreground.

The Apple Bluetooth Auto interface uses the public dual-role backend: it scans
and connects as a central, and advertises and accepts connections as a peripheral.
Configuration declares both `bluetooth-central` and `bluetooth-peripheral`
background modes and stable, distinct, variant-specific restoration identifiers.
The existing central identifier is retained. The same prepared manager owner is
handed to the native runtime; permission observation does not create another
manager or node. RemoteControl pairing remains a separate board-administration
operation, not permission to join the network.

Scene-based apps receive nil AppDelegate launch options, so `.bluetoothCentrals`
cannot be used to decide whether to recreate the managers. Each configured
process launch requests one restoration attempt with the same stable identifiers.
When Bluetooth permission is already allowed, the app delegate admits startup
and schedules preparation on the native queue without waiting for a scene or
React. Automatic startup never requests an undetermined permission in the
background. Under the supervisor lock, automatic
preparation first requires an existing storage directory and a valid primary
identity; fresh onboarding, malformed identity and reset-required state cannot
create a Bluetooth owner. Explicit user-driven Start remains unchanged.

Storage access and manager preparation do not block the main actor; authorization
and startup generation are checked before preparation and again before the full
native start. Stop/reset cancels the queued process-launch request and any unlock
retry. Unavailable protected storage can defer one recovery attempt until the
availability notification; ordinary relock does not preemptively block readable
storage. Scene disconnect, foregrounding and React reload do not request another
native start. Diagnostic `restorationAttempt`/`restorationAttemptRequested` fields
describe this attempt, not a confirmed Bluetooth wake or restored state.

That start consumes only the prepared owner with the same storage root,
restoration identifier, and Bluetooth identity instead of creating a duplicate
manager or Host. Restored central-role peripherals attach their delegates
immediately and retain bounded early callbacks while the Prns session is
rebuilt. An optional iOS Debug TCP fixture is embedded in the development
variant's `PRNSDevelopmentTcpTarget` metadata from
`EXPO_PUBLIC_PRNS_LXMF_TCP_TARGET` during prebuild. Rebuild the client to change
it; changing Metro's environment alone cannot reconfigure the process-owned
node. Native foreground and restoration attempts use the same target. Release
builds ignore this fixture. Non-Bluetooth interfaces still receive no additional
background execution entitlement.

The minimum deployment target remains iOS 18.0; removing AccessorySetupKit does
not lower or requalify it. Apple's
[TN3115](https://developer.apple.com/documentation/technotes/tn3115-bluetooth-state-restoration-app-relaunch-rules)
ties its iOS 26 AccessorySetupKit condition to specific relaunch cases; an
[Apple engineering clarification](https://developer.apple.com/forums/thread/806013)
limits the ASK-specific exception to user force-quit and Control Center Bluetooth
toggle cases. Ordinary background event delivery and state restoration remain
available without ASK. The app accepts losing those additional ASK recovery
cases to support automatic, unpaired phone networking. The historical
[current checkpoint](../checkpoints/2026-09-09-follow-up.md) records a bounded
restoration-requested relaunch and its remaining UI, delivery and lifecycle
limits. The full physical matrix remains open; source, simulator, build and
ordinary foreground results do not establish it.

The [BLE-only phone demo](phone-node-demo.md) starts by qualifying this ordinary
CoreBluetooth path. New foreground, background and restoration evidence is
required; earlier ASK results do not qualify the replacement.

During an iOS-granted Bluetooth background window, the process-owned Host and
Bluetooth interface can run without React. This is bounded, event-driven iOS
background behavior, not a continuously scheduled daemon. iOS still decides
when the process may run, suspend, wake, terminate, or relaunch, and a matching
pending Bluetooth operation and corresponding event are required for
restoration relaunch. Non-Bluetooth interfaces do not gain background execution
from `bluetooth-central`; reliable background message delivery remains
unadvertised until the complete physical lifecycle has passed.

Apple documents that a passcode-protected device will not
restoration-relaunch an app until after the first unlock after reboot. The
protected-data deferral remains defense in depth for any other pre-unlock launch
path; it is not an expected CoreBluetooth restoration path.

Debug iOS builds include a private Bluetooth log probe for that qualification.
It maps existing scan, connection, subscription, disconnection, and central
restoration milestones to bounded, payload-free event codes tagged
`PRNS_IOS_RESTORATION` in the device console and unified-log messages. Filter by
that message tag, not a custom log category. It does not expose peer addresses,
messages, pairing codes, accessory identifiers, or restoration identifiers, and
Release builds do not enable it. It adds no polling or background keepalive.
Scan events distinguish requests, queued work, state queries, decisions, and
completed calls. An already-scanning decision records a framework state query,
not fresh discovery progress. Sightings occur after admission, so these events
do not prove that every raw discovery callback was observed. Greeting events
distinguish a completed Hello write from a received Welcome. In a central-role
session, Hello means the acknowledged GATT write completed; neither
event alone proves a validated, settled handshake. Closed-session reaping records
the local cancellation path, not its cause or a measured handshake timeout.
Only static event codes leave the probe; control fields are not exported.
Reset/reconnect request markers identify the one-shot restored-native recovery
path, not a guarantee that the physical connection has closed or reopened.
The classifier and Swift
allowlist tests run in the explicit macOS `native:ios:test` gate without enabling
radio logging; portable checks also verify their source-level integration.

For startup capture on an already installed Debug build, start a PID-independent
stream before the lifecycle action:

```sh
idevicesyslog --udid <device-udid> --no-colors --exit --match PRNS_IOS_
```

Add `--network` (`-n`) when using the paired network transport. Verify the
connection and the new PID's launch plus `sequence=1` probe markers before
treating the startup capture as complete. Prefer USB for qualification and start
a fresh capture for each trial. A connected header or a still-running logger
does not prove continued reception: a network capture's internal reader was
observed failing and exiting while its command remained alive. A lost reader
requires a new capture, and the resulting gap cannot be treated as silence from
the app. PID-independent captures have both captured and missed relaunches;
the earlier process-filtered capture is not the only missing-timeline case.
`devicectl --console` alone is insufficient: it connects standard streams only
when launching a new process, and signals sent to it can reach the app. Logging
and a Home-screen observation do not establish natural suspension or exclude
observer effects.

A historical signed probe remains important negative evidence: after a clean
AccessorySetupKit activation, constructing the former dual-role backend's
`CBPeripheralManager` reproducibly aborted the unlocked iOS 26.6.1 process.
That result explained the previous central-only ASK composition; it does not
apply as evidence against dual-role operation without ASK. Apple confirms that
[declaring ASK changes the app-wide authorization model](https://developer.apple.com/forums/thread/797137)
and prevents peripheral-manager authorization. All ASK adoption keys and imports
are therefore removed, not merely bypassed at runtime.

Peripheral service restoration is not a restored Prns peer session. The backend
can recover its GATT service/characteristics, but a surviving central must
reconnect or repeat the Prns handshake before sending session data. Qualify
central-role and peripheral-role recovery separately; neither a restored service
nor foreground connectivity proves process-death message delivery.

The public [validation summary](validation.md) separates automated
checks, historical device observations, and the remaining lifecycle work.

## Physical iOS development client

Build and install a Debug development client on one explicitly selected iPhone:

```sh
PRNS_IOS_DEVICE_UDID=<device-udid> \
PRNS_IOS_DEVELOPMENT_TEAM=<team-id> \
PRNS_IOS_METRO_PORT=8088 \
  npm --prefix applications run native:ios:device
```

Automatic signing with permission to update profiles is the default. To limit
Xcode to profiles already installed on the Mac, provide the expected profile
UUID. The helper omits developer-portal permission and refuses to install the
app unless that exact profile is embedded:

```sh
PRNS_IOS_EXPECTED_PROVISIONING_PROFILE_UUID=<profile-uuid> \
PRNS_IOS_DEVICE_UDID=<device-udid> \
PRNS_IOS_DEVELOPMENT_TEAM=<team-id> \
PRNS_IOS_METRO_PORT=8088 \
  npm --prefix applications run native:ios:device
```

The helper does not launch the app. It stops before installation unless the
compiled app has bundle identifier `rs.reticulum.prns.dev`, the requested Metro
port, and a nonempty `ip.txt`. Start Metro on the same LAN and port before
launching the installed app:

```sh
npm --prefix applications/prns/app run start -- --lan --port 8088
```

This Debug build has no embedded JavaScript bundle, so it requires that Metro
session to load the UI. Open the installed `prns dev` app after Metro reports
that it is ready. This custom Debug app does not include `expo-dev-client`;
opening an `expo-development-client` deep link does not configure its bundle.
Expo reads the built `RCTMetroPort` and the host in the app's `ip.txt` instead.

If the app already shows **No script URL provided**, starting Metro alone does
not retry the failed launch. Once Metro is ready, reload from the developer
menu or restart the failed process:

```sh
xcrun devicectl device process launch \
  --device <device-udid> \
  --terminate-existing --activate \
  rs.reticulum.prns.dev
```

Use that restart only for launch troubleshooting. During a same-process
background/resume check, preserve the running process and foreground the app
normally without a terminating launch command.
