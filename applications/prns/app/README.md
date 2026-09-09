# Prns development app

This package is the clean Expo consumer for the application-owned native
aggregate and reusable application services under `applications/`. Base Prns
packages do not depend on it.

For dependency installation, package ownership, generation and verification,
start with the [application workspace guide](../../README.md).

## Current development route overrides

The 33 stable screen IDs from the product contract remain unchanged. The real
development slices use the identifiers they actually expose instead of
inventing future record IDs:

- onboarding uses `create` and `import` steps;
- contact details use a destination hash, and the manual-add route has no
  source parameter;
- conversations and direct-message composition use a destination hash; and
- a managed node's current `nodeId` value is its target identity fingerprint.

When services expose generated contact, conversation, or managed-node record
IDs, adopt the product-contract parameter types and update the route catalog,
links, and deep-link tests together.

## Finding a paired node

Saved pairing is authorization, not a live route. Before a connection check or
address-sharing operation opens its control Link, the native actor resolves
the authorized target and checks the route's actual interface for online,
transmit-capable status. Describe waits within the remainder of its original
20-second admission deadline; address sharing keeps its five-second readiness
wait. Readiness does not extend Describe's overall deadline. A ready route skips
discovery; otherwise an available transmitting interface permits one path
request through Prns's public API.
That request awaits an accepted announcement for the exact destination, then
the app checks the route's interface again before connecting. A stored route
alone is insufficient, and failure does not submit a remote command. These
checks use the status published by the app's Bluetooth/TCP transports, not a
general readiness guarantee for custom interfaces.

Prns owns the bitrate-aware discovery timeout and interface selection. Describe
has one 20-second deadline starting at admission, including queueing, readiness,
discovery, connection, and response time. Deadline expiry, native caller
departure, or priority Stop drops the app-owned Describe future and releases
its active-operation and admission state. An expired queued command never
starts network work. Generated async bindings accept an `AbortSignal`; the SDK
forwards it, including Effect interruption, so JavaScript caller cancellation
also releases Describe's app-owned future without stopping the native node.

Once a target connection has been returned, an app-owned guard queues Link
closure on completion or cancellation. Earlier establishment/identification
may already have issued upstream work: dropping its waiter does not guarantee
immediate engine cancellation before that handle exists. Neither path discovery
nor the remote operation is automatically replayed. Address sharing retains its
asynchronous operation ID and unknown-outcome handling instead of adopting
Describe's caller-cancellation semantics. See the
[validation summary](../../docs/validation.md) for current recovery evidence
and limits.

## Sharing a paired node's address

Manage node offers **Share node address** after a successful connection check
confirms that the current pairing allows it. The native operation checks the
target's current permissions again before asking it to announce its configured
self-announcement destination; that destination is not necessarily its remote
control endpoint.

Submission returns an operation ID immediately. The existing native actor owns
the request and retains its latest result across screen remounts and native
stop/start within the same process. The record is not a durable history and is
cleared by development reset or process termination. If confirmation is lost,
the app reports an unknown outcome and never automatically repeats the request.
The target may already have announced even when its reply cannot be recovered.

Host tests cover admission/shutdown races, permission checks, response outcomes,
retained results, and independently observing the configured announcement over
the upstream TCP interface. In an earlier signed development build, two
fresh authenticated connection checks completed over the paired Bluetooth path,
and one **Share node address** request completed with a retained success result.
The Host then independently learned the node's configured announcement
destination as a direct one-hop Bluetooth route. This foreground physical check
used no manual node announcement, re-pairing, or reset. It does not establish
reliable background recovery; see the [validation summary](../../docs/validation.md).

## Android development

The Android app supports Android 10 (API 29) and newer. A foreground service
owns the same Rust runtime used by iOS, with Android Bluetooth and permission
handling. **Stop node** on Nodes or This device and the running-node
notification's **Stop** action use the same service-owned shutdown. In-app Stop
remains available when notifications are disabled. Closing a screen does not
stop the node, and ordinary navigation or resume after Stop does not restart it.
Select **Start node** explicitly to resume with the saved identity and data.

See [Android development](../../docs/android.md) for setup, permission behavior,
and the physical acceptance sequence. The Android 10 development checkpoint
covers the current iOS app's feature scope with the build-specific evidence and
qualification limits in the [validation summary](../../docs/validation.md).
To build a standalone development APK
with bundled JavaScript and no Metro requirement:

```sh
npm --prefix applications run native:android:standalone
```

This uses the development identifier and local debug signing, not production
distribution credentials. Check the [validation summary](../../docs/validation.md)
before treating background operation or a particular transport as qualified.

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
[validation summary](../../docs/validation.md) for later restoration observations
and their limits.

Native lifetime is process-owned rather than scoped to a consuming screen.
Low-frequency development snapshot refresh runs only on `/nodes` and `/inbox`
routes and their descendants. Other shell routes keep the node and event owner
alive without periodic snapshot reads.

The iOS app now has one process-wide `ASAccessorySession`, activated from the
app-delegate launch path before it allows Prns to create CoreBluetooth state.
Both an ordinary JavaScript start and a restoration-requested native start wait
until the session has activated and its canonical accessory roster contains at
least one authorized Bluetooth accessory. The system accessory chooser opens
only after an explicit user action while the app is in the foreground. That
system authorization is a separate step from the later secure Reticulum pairing
flow; the UI does not expose raw accessory or Bluetooth identifiers.

The app's Apple Bluetooth Auto interface is central-only. Its tracked iOS
configuration declares one stable, variant-specific central restoration
identifier and only the `bluetooth-central` background mode. It does not create
a `CBPeripheralManager`, advertise, accept inbound Bluetooth links, publish a
peripheral restoration identifier, or claim the L2CAP capability. Existing
Prns consumers keep the dual-role backend by default; the app deliberately
selects the additive central-only role with restoration.

On an eligible relaunch for the app's exact central identifier, the app delegate
activates the accessory session early and records the restoration request. Once
the authorized roster is ready and protected storage is available, it prepares
one central manager and queues the full native start without waiting for React.
That start consumes only the prepared owner with the same storage root,
restoration identifier, and Bluetooth identity instead of creating a duplicate
manager or Host. Restored central-role peripherals attach their delegates
immediately and retain bounded early callbacks while the Prns session is
rebuilt. The development TCP fixture remains a normal launch input and is
intentionally absent from a restoration start.

AccessorySetupKit requires iOS 18, so the Expo configuration plugin, Xcode
project, and native module all use iOS 18.0 as the minimum deployment target.
Starting in iOS 26, prior AccessorySetupKit setup is also a platform eligibility
gate for CoreBluetooth process restoration, including Apple's documented
force-quit exception. The source now implements that setup gate, but the
central-only path has not yet passed the physical background/restoration matrix
on physical hardware. Do not infer a successful restoration relaunch from source,
simulator, build, or foreground results.

During an iOS-granted Bluetooth background window, the process-owned Host and
central interface can run without React. This is bounded, event-driven iOS
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
do not prove that every raw discovery callback was observed. The classifier and Swift
allowlist tests run in the explicit macOS `native:ios:test` gate without enabling
radio logging; portable checks also verify their source-level integration.

A historical signed probe remains important negative evidence: after a clean
AccessorySetupKit activation, constructing the former dual-role backend's
`CBPeripheralManager` reproducibly aborted the unlocked iOS 26.6.1 process.
That result is why the app is central-only. It did not authorize an accessory
or exercise the current central-only implementation, so it is not positive
physical evidence for the architecture described above. This implementation
uses one central restoration identifier, not a dual-role configuration.

The public [validation summary](../../docs/validation.md) separates automated
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
