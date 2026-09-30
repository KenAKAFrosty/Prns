# SDK migration mobile validation

This checkpoint records September 23, 2026 device work on the uncommitted
`prns-app` working tree based on `0461d7aadc57`. It supplements the
[SDK implementation record](../docs/react-native-sdk-implementation.md). Build
success, retained-data checks, and physical reload qualification are reported
separately below. Nothing was committed or pushed for these trials.

## Status

| Area | Observed result |
| --- | --- |
| Galaxy standalone install and local node lifecycle | Passed with retained data on the first SDK-migration Release APK |
| Galaxy Bluetooth off/on recovery | Passed in Debug; node process, foreground service, and identities survived |
| Independent SDK host ownership on both phones | Passed in actual Hermes after fixing the SDK attachment import |
| JavaScript reload with a pending diagnostic reader | Initial failure on both phones; native-guard rebuild passes on both phones |
| E290 firmware update | Sparse flash verified; inspected identities, settings, grants, and existing journal records preserved |
| iPhone retained E290 read | Initial route failure; later public SDK Describe succeeded with the preserved grant, 322 ms RTT |
| Bilateral LXMF between the phones | Both directions delivered with matching verified inbound records after current native rebuilds |
| Final standalone installs | Both current native-guard Release apps installed with retained data and running without Metro |
| Bounded Home-screen receipt | Standalone iPhone acknowledged and retained one message while Home; this does not qualify long idle or OS restoration |

Both apps use the aggregate `prns-app` provider. The Galaxy APK contains one
selected PRNS image, `libprns_app.so`; the signed iPhone app embeds one
`prns_app.framework`. The shared SDK packages that image. These observations do
not qualify the independent SDK provider on either phone.

## Galaxy S9+

The connected device is a Samsung SM-G965U running Android 10/API 29. Installs
used the existing development application identifier and retained application
data. No uninstall, application-data clear, identity reset, pairing replacement,
or permission-grant clear was performed. The device remained USB-powered with
its existing display and text-size settings.

The first standalone Release APK was built from the current aggregate native
provider and included its JavaScript bundle. Its SHA-256 was
`e69dc3087b3e0c6cea85ae22479f91e6e2e381950e4ca4daf8263969ae8fa720`.
The helper completed its 16 KiB APK alignment check. After a retained-data
install, the UI showed a running node and Bluetooth ready; primary/controller
identities, the saved pairing, and the existing inbox threads with four and
three messages remained present.

The in-app Stop action removed the foreground service. Saved messages remained
readable while stopped, and returning from the Home screen did not restart the
explicitly stopped node. Explicit Start restored the running node, foreground
service, and saved pairing. This covers deliberate local lifecycle actions,
not Android process death, a phone reboot, Doze, or long idle operation.

A Gradle-only Debug build then passed all 35 shared SDK Android JVM tests and
was installed with data retained. In Android Settings, Bluetooth was disabled
and enabled once. The app changed from **Ready** to **Needs attention**, then
returned to **Ready**. The same process, service instance, foreground
notification, and primary/controller identities survived. Bluetooth was left
enabled. This is a bounded adapter recovery observation, not a successful
authenticated remote read or a long-disconnection test.

The final standalone native-guard Release was subsequently installed with data
retained. The installed APK SHA-256 matched the built artifact
`414f662cc9943b55db5ac5557c10c32ca8b4cbf6c7dd0cd171e065fc42accf75`.
With Metro stopped and the temporary USB reverse mapping removed, the app
started **Running/Ready**. Direct UI inspection confirmed unchanged primary and
controller identities, original pairing, and inbox counts of seven and three
(the original messages plus these trials). Bluetooth remained enabled and the
foreground service remained active.

The Galaxy's retained pairing targets a different board from the connected E290.
An early timeout therefore does not test this board's retained authorization or
establish an SDK transport regression. The pairing was preserved rather than
replaced for this trial.

## iPhone

The selected iPhone was running iOS 27.0. Device control and installation used
CoreDevice's observed local-network transport; the USB inventory was empty.
The baseline contained the running persisted/restored node, its identities,
the saved E290 pairing, and inbox conversations containing four and thirteen
messages.

The current Debug app built, signed, and installed with retained data. The UI
showed a running node and the saved target. Its embedded aggregate framework
SHA-256 was
`933d7ddc33a5811af4d29478f1de68e4a352f98e3c3d6c72209d63209e8f7755`.
The bundle retained both Bluetooth background modes and the iOS 18 deployment
minimum. This metadata does not establish background behavior. After the Debug
install and Hermes reload, direct UI inspection confirmed persisted/restored
state, unchanged primary/controller identities, and retained inbox counts of
four and thirteen messages. The SDK reload baseline also read two contacts and
seventeen messages. Automatic Bluetooth LE showed **Connected**, but traffic
and route counters were zero; that label does not prove a usable message route.

The iPhone's saved target matches the connected E290 and its preserved grant.
After firmware readiness, one read-only Describe through the canonical host
request API returned the typed failure `NoRouteToDestination`; the process-owned
host remained running. This direct API connects without first discovering a
route. The app's **Check node connection** action separately waits for transport
readiness and requests a path before connecting. The direct failure therefore
does not establish failure of that discovery flow or successful management
acceptance. A later direct UI attempt through **Check node connection** also
reported that the saved target was not reachable. Bluetooth control handshakes
are excluded from the displayed Prns data-byte counters, and startup does not
automatically announce LXMF. Read-only Galaxy logs additionally showed repeated
subscribe/control exchanges followed by policy-driven connection closure with
one peer. The log did not establish that peer's identity or the specific closure
reason. Later explicit announcements and verified bilateral delivery established
a usable phone-to-phone route, as recorded below. The final standalone Release
was rebuilt with the native object guard, installed with data retained, and
launched without Metro. Its inbox retained the original four-message peer
thread plus the two new acceptance messages and the thirteen-message offline
thread. The Release binary SHA-256 is
`e713e1291d3bd3b35afc650fe7bdef265647ef92425ea4910c8c890b21732238`;
the bundled JavaScript SHA-256 is
`2d328025e182204eb074c6945ef5a68e43ffb3d7e01e7b0065a8915ad453ddec`.

## Actual Hermes ownership and reload probes

The probes use the app's public platform attachment and SDK host APIs in the
installed Debug applications. They do not stop the product-owned node or reset
its storage. Temporary owned hosts use ephemeral persistence and no attached
interfaces.

Both phones first exposed an SDK attachment bug: the platform's lazy import of
the SDK wrapper generated a Metro chunk path outside the application workspace.
The native binding entry now statically exports `borrowHost`, and the platform
uses that already-loaded entry. Both phones subsequently attached successfully.
The focused platform typecheck, formatting/lint, and 63 tests passed for that
correction.

On both phones, an independent ephemeral SDK host ran with a different identity.
Closing pending event readiness settled its iterator, the application-event
reader could be reclaimed, and concurrent Stop/Close calls shared completion.
Stopping that temporary host left the product generation, identity, and running
state unchanged. Borrowed product handles exposed neither Stop nor Close and
could not steal the native application's already-claimed event lane.

The reload trial deliberately held a diagnostic reader with a pending readiness
future. The debugger observed the JavaScript execution context being cleared
and recreated. The replacement context had no old probe marker, while the same
native generation and identity survived with increasing uptime. However,
reclaiming the diagnostic reader returned `AlreadyClaimed` on both phones; the
Galaxy repeated the failure after a further delay.

Both actual Hermes runtimes reported `FinalizationRegistry` as unavailable.
Generated object wrappers used that optional JavaScript registry to release
their native references. The existing native teardown handled pending futures,
but not the original object reference that retained the stream. The Galaxy's
native log confirms teardown ran after its JavaScript thread terminated; both
players reported two completed root teardowns. The pinned UBRN runtime/generator
now includes native lifetime guards in
[`0006-jsi-native-object-lifetime.patch`](../../vendor/ubrn/patches/0006-jsi-native-object-lifetime.patch).
The runtime archives were rebuilt and their six integrity entries refreshed
across the application, SDK, and example lockfiles. All three clean installs
passed, and every installed runtime/core package file matched its sealed archive.
The regenerated SDK passed typechecking, 18 Node tests, and three Python tests;
the platform source and test typechecks passed. Those automated checks alone
did not qualify phone reload recovery; the physical retests are recorded below.

The sealed runtime source tree is `8296380d415b239a22b17b8feef7a101c6f300df`,
with source-lock SHA-256
`69ec9b87ad1411c9ae525398af4988be88cb77ea7f82f33e9c5d4823877d75ba`.
It was built with Xcode 27.0/27A266a. The desktop Hermes harness matches React
Native's Hermes revision `829dcd74` (v0.17.0). The new object-lifetime fixture
passed five runtime replacements, covering collection, explicit destruction,
multiple wrappers sharing a pointer, and pending methods. All six reload
fixtures also passed their normal two-runtime configuration. The separate
joined-queue configuration passed all six fixtures with two contexts each. A
negative control that disabled the new native blessing entry reproduced the
object leak and failed the fixture. These desktop checks are distinct from the
subsequent phone acceptance below.

The Galaxy Debug and Release apps were rebuilt against the sealed runtime
archives and regenerated bindings. Both APKs contain the native object-guard
implementation and only `libprns_app.so` as their PRNS image. The Debug APK
SHA-256 is `74a5d8bc89111d65f2d6999ae3526c153a1e18804f4c5ebdbbe788a4c5795336`;
the standalone Release APK SHA-256 is
`414f662cc9943b55db5ac5557c10c32ca8b4cbf6c7dd0cd171e065fc42accf75`.
Debug was installed with retained data. Its owned-host probe passed again.
Repeating the pending diagnostic read and genuine Hermes reload now successfully
reclaimed the reader. The native generation and identity stayed unchanged,
uptime advanced from 14,656 to 34,540 ms with 1 ms elapsed-time drift, borrowed
release preserved the running owner, and all seven previous mailbox IDs stayed
unchanged. The rebuilt iPhone also passed pending-reader reclamation; its
independent owned-host probe passed again. Its native generation and identity
were unchanged, uptime advanced from 39,391 to 66,602 ms with 4 ms elapsed-time
drift, both contact destinations and pins and all seventeen previous mailbox IDs
were retained, and borrowed release preserved the running owner. The product
application lane remained claimed by its native owner. Both final standalone
installations are recorded above.

## Bilateral LXMF

With the rebuilt Debug apps, both phones explicitly announced through the
product LXMF action and the public SDK host command. Each discovered the other
phone's verified current identity and LXMF destination on a fresh one-hop route.
The Galaxy's only attached interface was Automatic Bluetooth LE; its data
counters advanced from zero after these announcements.

Inspector attachment alone did not establish visible foreground state: later
iPhone Mirroring showed the Home screen. The iPhone was visibly brought to
**Nodes — Running** at 21:01 UTC before the following deliveries. The Galaxy
remained visibly foreground. One acceptance message was sent in each direction:

| Label | Sender result | Receiver result |
| --- | --- | --- |
| `SDK0923-iPhone-2102` | Delivered, 175 ms RTT | Galaxy recorded the same message ID as verified inbound |
| `SDK0923-Galaxy-2102` | Delivered, 58 ms RTT | iPhone recorded the same message ID as verified inbound |

The Galaxy retained its native generation and identity during these sends. An
earlier temporary inspector-helper failure occurred during message-list
preflight before submission; the abandoned `2058` label was never sent. This
foreground exchange does not qualify locked or long-idle delivery.

After the current rebuild and route discovery, the iPhone public SDK Describe
request succeeded against the connected E290 with its preserved grant, 322 ms
RTT. This supersedes the earlier no-route result for this specific read. It
does not establish permission for extended management queries or a fresh
pairing flow.

## Bounded standalone Home-screen receipt

The iPhone standalone Release was visibly placed on Home at 21:04:29 UTC, with
no Metro or inspector attached. The Galaxy sent `SDK0923-ReleaseHome-2104` once
at 21:04:47 UTC. The sender recorded Delivered with a 56 ms RTT while the
iPhone remained Home. At about 21:05:25 UTC, resumed iPhone UI showed the label
as Received and its phone conversation increased from six to seven messages.
The same iPhone process survived. This is a short unlocked Home-screen check.

The first attempted iPhone UI reply, `SDK0923RESUME`, was rejected locally
because the address was not ready to receive messages; no outbound record was
created. Outbound peer discovery had not been established after the Release
install, so this does not establish a background regression. After the Galaxy
Release UI explicitly shared its messaging address, the retained unsent draft
was submitted once and Delivered in 234 ms. The Galaxy UI confirmed Received
and its phone conversation increased from seven to eight messages.

The optional reciprocal `SDK0923ANDROIDHOME` trial was not run: iPhone
Mirroring ended because the physical phone was in use, before any test message
was sent. The Galaxy waited on Home and was then returned to **Nodes —
Running**, with Bluetooth **Ready**, its original pairing, and the same process.
No Android offscreen message-receipt pass is claimed for that wait.

## E290 firmware and preservation

The connected Heltec Vision Master E290-HF V0.3.1 had September 8 firmware whose
pairing protocol predates the current app. Its current firmware was built from
base `0461d7aadc571ad073499c8644f318998ea0b12f` plus the recorded working-tree
changes. The relevant source diff SHA-256 was
`bbfbc916f095e6211d07c72e073ce6ae3c262e275ca2b0e6e935736bd0869eb7`;
the flashed application SHA-256 was
`36e0891047b89ef60f8c09bca2ef949a34edaf907d18750d2ae5935974e8f45b`.
The short source label printed at boot alone does not identify that dirty build.

The supported sparse-flash flow verified three written parts totaling 2,347,888
bytes, without a full-chip erase. Protected pre/post reads confirmed byte-for-byte
preservation of BLE, node and remote-control identities; Wi-Fi/TCP provisioning;
radio configuration; and all original active journal records, including grants.
Both old and current readers selected arena A, epoch zero, with 101 valid
records. The newly reserved pages and arena B were erased, so this particular
board required no journal relocation. This does not qualify upgrades of boards
whose old arena B is active, because that boundary moved.

The updated board booted, restored 75 routes without a restore warning, loaded
its saved Wi-Fi provisioning, and accepted native Bluetooth GATT with MTU 508,
data length 251, and Le2M PHY. Its existing configured TCP connection still
returned `ConnectionReset`, as before the update; that setting was not changed.
Bluetooth link acceptance alone does not prove a remote-control request or an
LXMF delivery succeeded.

Both saved grants retain only Describe and AnnounceSelf permission, mapped to
Operator authority by the current reader. Extended settings, inventory, build,
or grant-management operations may correctly be denied. No grant was added or
expanded and no fresh physical pairing was performed.

Opening USB serial during the attempted initial passive capture caused an
observed board reset. Subsequent reset/flash windows were coordinated with the
phone tests. Serial was closed after the final boot check; no later reset is
claimed as part of a completed phone acceptance check here.

## Evidence and remaining work

Compact receipts and screenshots remain outside the repository:

- `/tmp/prns-android-sdk-validation-20260923/`: build/install, retained lifecycle,
  Bluetooth service/UI, 35-test receipt, initial failing and rebuilt passing
  ownership/reload probes, final installed APK hash, labeled delivery receipts,
  and standalone retained-data screenshots.
- `/tmp/prns-sdk-mobile-20260923/ios/`: baseline, signed builds, current
  `guard-{owned,before,reload,after}.json`, and labeled bilateral message receipts.
  `release-home-summary.json` links the bounded standalone receipt to its sender
  delivery proof and matching before/after process snapshots.
  The additional post-reload inbox/lifecycle UI observations were captured
  through iPhone Mirroring in the task, without a separate saved screenshot.
- `/tmp/prns-iphone-hermes-{owned,before,reload,after-status,describe}.json` and
  `/tmp/prns-iphone-hermes-after.log`: physical Hermes evidence and typed failure.
- `/tmp/prns-sdk-mobile-20260923/e290/`: compact flash, preservation, journal,
  source-diff, and boot receipts. Protected backups contain credentials and key
  material; they must remain local and must not be published or attached.

Device identifiers, public identity values, pairing targets, and raw device logs
are kept in those local receipts. Long-idle delivery, locked delivery, OS
restoration, permission
denial, phone reboot, and two-iPhone interoperability remain outside this
checkpoint's completed evidence.
