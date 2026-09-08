# Application validation and current limits

This is an early development application, not a release-qualified client.
Base Prns changes are maintained separately for upstream review; their temporary
integration in this branch does not transfer application policy into Prns.

## Repeatable automated checks

Run from the repository root with the toolchains recorded in
[`release/compatibility.json`](../release/compatibility.json):

```sh
npm --prefix applications run verify
npm --prefix applications run mobility:verify
python3 validation/hygiene/application-boundary.py
python3 validation/hygiene/no-personal-paths.py HEAD
```

The ordinary application gate checks the compatibility record, generated Host
contract, native services, SDK, UI, route catalog, Expo configuration, and web
export. The detached gate exports the application into an independent directory,
resolves its exact recorded Prns revision and JavaScript artifact, and exercises
the native/SDK/UI and bidirectional native/Python LXMF checks there. This checks
the extraction boundary; it does not qualify a phone, background delivery, or
release packaging. The source checkout is checked for unintended base changes.

The base branch has focused Rust, JavaScript, browser, Bluetooth, persistence,
and firmware tests. Passing these is distinct from passing every repository
gate: the unchanged casework dependency smoke currently fails its matcher
contract assertions. Those assertions remain enabled.

## Historical physical observations

Development trials through September 7, 2026 used one iPhone 14 Pro on iOS
26.6.1 and an E290 node. These are observations of the signed builds used in
those trials, not fresh qualification of every later rebase. Device identities,
signing profiles, packet identifiers, and raw logs are kept outside the public
source tree.

- Accessory authorization, RemoteControl pairing, and fresh authenticated node
  checks completed. One address-sharing operation was independently observed
  as a direct Bluetooth route to the node's configured announcement destination.
- Reloading the UI preserved the native process and increasing Host uptime;
  a fresh authenticated node check then succeeded.
- A USB-observed trial recorded natural suspension and Bluetooth-attributed
  resumption. Native protocol work completed during an already-active background
  window. That does not prove the test packet caused the wake.
- After developer-induced non-user termination, with the phone locked and
  Metro off, public logs recorded an OS restoration launch, accessory-session
  readiness, native preparation/start, and restored central-session stages.
  The restored session disconnected and then connected again.
- In that controlled trial, the first packet received no proof before timeout.
  One path request was followed by a newly observed announcement; a second,
  distinct packet received a verified proof in 88 ms. The Mac node and USB
  connection remained the same. The first failure is retained, not counted as
  a successful first attempt.
- After returning to the foreground, the development UI was blank until an
  ordinary app restart. Pairing remained saved and a fresh node check succeeded;
  no reinstall, reset, or re-pairing was needed for that recovery.

The path request also generated traffic and introduced a delay. These results
do not establish stale routing as the sole cause, lossless callback handoff,
guaranteed wake, or reliable RemoteControl/LXMF recovery. The proof packet was
not an LXMF message. Developer-induced termination is not memory-pressure
eviction, an App Switcher force quit, or a pre-first-unlock test.

## Initial Android observations

September 8, 2026 bring-up used one Galaxy S9+ running Android 10 (API 29).
The application still has a development identifier and local debug signing.

- ARM64 debug and standalone development APKs build and package successfully.
  The standalone APK contains its JavaScript bundle, enforces API 29 as its
  minimum, and passes 16 KB packaging alignment checks; the Rust library's
  load segments also use 16 KB alignment.
- The standalone app opens with Metro stopped and no USB Metro forwarding.
  Its saved identity survives reinstalling the development APK. The foreground
  service remains present after leaving the app, and returning preserves the
  native process and increasing Host uptime.
- An on-device test loads the actual JNI library, checks the bridge contract
  and malformed requests, then preserves an identity and contact across a
  complete native stop/start. It runs in an isolated test application, not the
  interactive app's storage. This is not an OS process-death or Bluetooth test.
- The ordinary application gate passes, including 126 native unit tests, one
  controlled TCP lifecycle integration test, 69 SDK tests, 168 UI tests, Expo
  configuration/Doctor, and web export. Android builds additionally pass 27
  JVM adapter/lifecycle regressions and release lint. Host-based Android tests
  are now included in the ordinary native test command.
- The original Bluetooth startup repeatedly timed out after a 750 ms fallback
  began service discovery before Android completed MTU negotiation. Waiting for
  the matching callback allows the same phone to complete discovery and both
  subscriptions. The app subsequently receives the board's pairing announcement
  and sends pairing traffic over its Bluetooth-only composition.
- The first physical Android pairing attempt still timed out before confirmed
  pairing. No authenticated remote request or Android LXMF exchange has passed
  yet. The invitation's remaining time and packet counters are not sufficient
  evidence to attribute that failure to a particular cause.

The current Android source has not yet passed the detached mobility gate or
the full physical lifecycle matrix. Screen locking, permission changes,
radio/peer recovery, process eviction, background delivery, and newer Android
permission/service behavior remain unqualified. See [Android development](android.md)
for the repeatable acceptance sequence and the temporary runtime-restart behavior
when the Bluetooth listener changes.

## Remaining work

- Make stale-route recovery and caller-timeout cancellation deterministic in
  held-operation tests, then repeat the corresponding physical checks.
- Qualify locked/offline-peer recovery, repeated suspension/restoration,
  authorized force-quit behavior and its negative control, Bluetooth changes,
  and cold-start/storage boundaries. Include UI recovery and first-attempt
  delivery outcomes, not only native startup markers.
- Complete Android device coverage, broader board/transport
  coverage, release packaging, and migration/upgrade qualification.

The iOS implementation is not foreground-only: its process-owned Host and
central interface can run during Bluetooth execution windows granted by iOS.
It is not an always-running daemon, and other interfaces do not gain background
execution merely because the app declares `bluetooth-central`.
