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
- Foreground Bluetooth access and the separate background-discovery location
  grant complete through Android's permission dialogs. This verifies the
  permission flow, not discovery or delivery while the phone is locked.
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
  pairing. No authenticated remote request had passed at that point. The
  invitation's remaining time and packet counters are not sufficient
  evidence to attribute that failure to a particular cause.
- Investigation reproduced an Embassy Bluetooth receiver defect: joined frames
  lost their trailing frames, and frames split across L2CAP reads were discarded.
  The shared stream-deframing correction passes 12 receiver regressions, the
  full 185-test Embassy suite, strict Clippy, and two no-std target checks.
  Integrated firmware `735859ffba03` was built, flashed without erasing settings,
  and booted successfully on the USB-connected E290. The first fresh invitation
  with that firmware still timed out before confirmation. Its USB monitor had
  disconnected before the attempt, so it provides no request-ingress evidence.
  The framing defect is fixed; those physical pairing failures are not
  individually explained by the available evidence.
- A later fresh invitation reached the confirmation step on the Galaxy and
  board, and the user confirmed matching codes. Pressing the board button to
  select acceptance was followed by a freeze and restart. The phone was not
  approved, and no grant was confirmed in that attempt. That trial reached
  confirmation but did not complete pairing.
- Source tests also exposed an invitation-window mismatch: the target rejected
  a new attempt when its configured confirmation timeout no longer fit, even
  while the invitation remained open. The target now signs a timeout bounded
  by the remaining window, without extending approval or authorization expiry.
  The combined app-tree RemoteControl test subset passes (186 tests). The
  isolated correction also passes no-std/alloc checks and strict Clippy; its
  registered Kani proof has not run because `cargo-kani` is unavailable.
- A subsequent fresh pairing completed on both devices with diagnostic firmware
  based on `b3f8fe243434` plus temporary button/reset logging and DEBUG enabled.
  The app showed **Paired**, listed the target in Nodes, and its first read-only
  **Check node connection** returned **Connected** in 190 ms. The board log
  recorded the pairing offer, both button events and successful display
  refreshes, controller commit, and the later request forwarded to the
  application. This passes one foreground Android Bluetooth pairing and
  authenticated Describe exchange. No reboot or watchdog warning occurred in
  that captured run. The earlier button-triggered freeze remains unresolved;
  success with extra logging is not evidence that it is fixed.
- The installed standalone app retained its identities and pairing after an
  explicit Android force-stop and cold relaunch into a new process. Bluetooth
  reconnected and the first authenticated Describe returned in 189 ms. A
  subsequent deliberate USB reset of the board also preserved usable
  authorization: after Bluetooth reconnected, a fresh Describe returned in
  199 ms without re-pairing or restarting the phone app. This checks an app
  process restart and a board reset, not a phone OS reboot or sudden power loss.
- During a short screen-off, secure-keyguard observation, a second deliberate
  board reset was followed by automatic Bluetooth discovery, subscriptions,
  and L2CAP reconnection in the same Android process. The foreground service
  remained active. After the user unlocked the phone, an authenticated Describe
  returned in 191 ms. The first post-unlock request reached the board but its
  UI result was not captured; the recorded success is the repeat, not proof of
  first-attempt delivery. The phone was USB-powered and not in Doze. This
  demonstrates short locked transport recovery, not locked message delivery,
  long-idle reliability, or battery-powered behavior.
- A foreground LXMF exchange passed over the actual installed Android app's
  Bluetooth-only interface, through the board's existing TCP connection, to a
  pinned Python peer on the development Mac. Python 3.13.7, RNS 1.5.2, and
  LXMF 1.1.0 used the recorded dependency pins. The fixture was restricted to
  the phone's exact messaging destination. The phone received `python-to-rust`
  with **Verified source**, then sent `rust-to-python`; the peer verified the
  source and received its outbound delivery proof. The app showed **Delivered
  in 106 ms**. Both message IDs, contents, verification, and delivery state
  remained after another explicit app force-stop/cold relaunch, with the test
  peer already stopped. This qualifies one
  small direct-message exchange in both directions and app-process persistence,
  not Resources, propagation, locked delivery, or phone OS reboot durability.
- A later delayed inbound test obtained a proof for a new LXMF message while
  the Galaxy remained securely locked with its screen off. The same Android
  process and foreground service were present before and after. After unlock,
  the real inbox showed the exact proof-matched message ID, content, **Verified
  source**, and **Received** state. Those fields also survived an explicit app
  force-stop and cold relaunch into a new process, with the peer already stopped.
  This establishes short locked delivery on USB power and app-process mailbox
  retention, not Doze, battery-powered delivery, notifications, or phone OS
  reboot durability. No phone reply was attempted in this run, so the combined
  bidirectional fixture reached its deadline despite the successful inbound
  delivery; it did not produce a combined success result. The delayed fixture's
  26 focused tests pass.
- A fresh peer subsequently received a new message's delivery proof after the
  user reported unplugging the Galaxy. USB device observations before and after
  the proof, plus the bounded connection observer, showed the phone absent.
  The peer first discovered the exact phone destination with explicit path
  requests through the board. After reconnection, Android's battery history
  showed discharging with no charger, screen off, and `device_idle=light`
  spanning both submission and proof. The proof was about 54 seconds after
  light-idle entry; charging and screen-on transitions occurred much later.
  The inbox showed the exact proof-matched message ID, content, **Verified
  source**, and **Received** state. All remained after an explicit app
  force-stop/cold relaunch into a new process, with the peer already stopped.
  This qualifies one battery-powered, screen-off delivery during Android-reported
  light idle, plus app-process mailbox retention. It does not establish secure
  keyguard state, deep Doze, prolonged idle, notifications, or power-loss
  durability. The sender was stopped deliberately after the one-way proof,
  without attempting a phone reply: this is not a combined bidirectional pass
  or passive route recovery. The discovery helper's 30 focused tests pass;
  host tests do not opt into physical-peer path discovery.
- At the start of that later test, before any agent-requested reset or flash,
  the board's current boot reported `SysRtcWdt` as its last reset cause. This
  identifies an RTC watchdog reset but not the stalled task or triggering action.
  The capture began after the reset; repeated health samples are the same
  latched cause, not evidence of repeated resets. The prior button-freeze issue
  remains open.

The detached mobility gate passes for application commit `789aec2e1` against
the recorded Prns revision. This includes exact dependency resolution,
generated contracts, the application gate, and bidirectional native/Python
LXMF delivery with verified proofs. It does not build an Android APK or run
that exchange on the phone.

## September 8 upstream integration checkpoint

`prns-app` integrates upstream `1d4d4ba86` at merge commit `f3a9e9863`, including
the Tokio wake-ordering fixes. The merge preserves request-ingress diagnostics
and both feature-gated forwarding representations. Existing shared history and
published prerequisite branches were not rewritten.

- Core tests pass in both default and movable-forwarding configurations:
  1,896 passed and three ignored in each. Tokio passes 234 default and 247
  all-feature tests, with strict Clippy in both configurations. The compression
  subset passes 20 tests; core alloc-only cross-compilation and the runtime's
  alloc/external-allocation movable-forwarding check pass.
- The generated unsafe inventory was reconciled with existing application
  prerequisites omitted from the older baseline, including 14 existing
  CoreBluetooth blocks. No policy exception, scanner, dependency version, or
  runtime code changed as part of that reconciliation; it is inventory
  maintenance, not a new memory-safety proof. The ordinary audit check then
  passed against the refreshed baseline.
- The full application gate passes on the integrated source, including 126
  native tests, the controlled TCP lifecycle test, 69 SDK tests, 168 UI tests,
  Expo Doctor, and web export. The first attempt stopped at an unrelated local
  CocoaPods executable failure; selecting the existing working installation
  for the command resolved it without changing global tools or disabling checks.
- A local qualification-only child, `87b3cfc88`, pins the combined runtime
  `f3a9e9863` and passes the detached mobility gate, including exact dependency
  resolution, unchanged contract/artifact hashes, source preservation, and
  bidirectional native/Python LXMF delivery. The main branch's published
  compatibility pin remains `ebed61ce3`; the new local qualification does not
  claim remote availability of its candidate or a published pin update.
- A newly built standalone Android APK was installed without clearing data.
  Pairing remained usable, its first submitted authenticated node check returned
  **Connected** in 208 ms, and the earlier battery-trial message remained with
  its exact ID, content, verification, and received state. This is a foreground
  post-update check, not renewed background or full device qualification.
- The board was flashed with firmware based on `f3a9e9863` plus the archived
  temporary watchdog/display diagnostics. Sparse verification passed and
  settings were retained. Both core heartbeat sequences, feed decisions, and
  completed refresh phases are visible, and the authenticated request reached
  the board. The temporary source patch was then removed from the checkout;
  the installed diagnostic image is explicitly not a clean-tree release image.
  No watchdog timing or button policy was changed. The intermittent freeze
  remains unresolved pending a captured reproduction.
- The user subsequently reported that the requested menu check seemed normal,
  with no freeze. Separately, the bounded 20-minute diagnostic capture completed
  with advancing core heartbeats, successful watchdog feeds, and successful
  display refreshes; no stall or subsequent reset was recorded. No button
  markers were captured, so the manual check cannot be correlated with that
  log window. This is a successful user-reported check, not proof that the
  intermittent freeze is fixed.

## Android lifecycle follow-up

The same standalone Android build and installed diagnostic board image were
used for one foreground, USB-powered lifecycle sequence. No app data was cleared
and neither device was reflashed for these checks.

- **Explicit Stop:** the notification action removed the foreground service
  and stopped the native Host without ending the app process. Navigation and
  ordinary background/resume did not restart it. A force-stop/cold relaunch
  restored the saved pairing; its first authenticated node check succeeded in
  184 ms. This exposed a recovery UI gap: there is no in-app Start control, and
  the stopped snapshot's unavailable inventory is misleadingly shown as
  **No paired nodes**. A cold relaunch is a development workaround, not the
  intended user-facing restart flow.
- **Bluetooth off/on:** Android Settings disabled Bluetooth, the app showed
  **Needs attention**, and a node check failed while it was disabled. The
  foreground service and app process remained alive. After enabling Bluetooth,
  the first submitted check succeeded in 244 ms without a manual app restart.
  Replacement listener PSMs were published during recovery; the implementation
  explicitly restarts its native generation when the PSM changes. This does
  not qualify uninterrupted TCP connections.
- **Location permission revoke/regrant:** denying Location in Android Settings
  revoked foreground and background discovery access. Android explicitly
  killed the process for permission revocation and restarted its sticky service.
  The app reopened with Bluetooth unavailable and an explicit **Allow Bluetooth**
  action, without an unsolicited permission prompt. Granting foreground access
  through that action restored discovery but left background access denied.
  The first immediately submitted node check failed; a later manual retry
  succeeded in 133 ms. This is recovery with a failed first attempt, not a
  first-attempt pass. The diagnostic follow-up below identifies the failing
  protocol stage without establishing its route-selection cause.
- Background discovery was restored through its separate permission prompt.
  Final OS grants matched the starting state, Bluetooth was enabled, and a
  final authenticated check succeeded in 195 ms. The earlier battery-trial
  message retained its exact ID, content, **Verified source**, and **Received**
  state. The temporary UI helper was removed afterward.

The board capture recorded successful request ingress and ten successful
display refreshes with healthy heartbeat/feed samples and no recorded reset
or stall. Captures were stopped explicitly after the tests. No new physical
message exchange, battery-idle delivery, or button exercise is claimed here.

The full physical lifecycle matrix remains pending beyond these narrow checks.
First-attempt connection readiness needs follow-up;
repeated transitions, prolonged peer loss, arbitrary OS process eviction, deep
Doze or long-idle delivery, system Location off/on, and newer Android
permission/service behavior remain unqualified. See [Android development](android.md)
for the repeatable acceptance sequence and the temporary runtime-restart behavior
when the Bluetooth listener changes.

### Explicit in-app restart

The follow-up standalone build adds **Start node** on Nodes and This device,
with serialized observer cleanup, duplicate-press prevention, and rejection of
late results from a replaced session. Stopped inventory is now shown as
unavailable rather than empty. Service status changes and ordinary resume do
not start a stopped node. The full app gate passed: native tests 126 plus one
controlled TCP lifecycle test, SDK tests 69, UI tests 184, and Expo Doctor 21/21,
along with contract, bridge, type, lint, format, configuration, and web checks.
The standalone build and 27 Android JVM tests also passed.

On the USB-powered Galaxy S9+, notification Stop removed the service while
preserving the app process. Nodes and This device offered **Start node**;
navigation and Home/resume left the service stopped. Pressing Start on This
device restored the service and native Host in that same process. The primary
and controller identities were unchanged, the saved pairing returned, and the
first submitted authenticated check succeeded in 145 ms. The earlier battery
trial message retained its exact ID, content, **Verified source**, and
**Received** state. No new message exchange or idle delivery was attempted.

The first physical pass also exposed a paired-node detail page showing **Not
found** when Stop cleared the current inventory. A further correction preserves
that route with local-node status and **Back to Nodes** guidance, while genuinely
missing pairings are still rejected with a running inventory. Android guidance
offers Start; iOS guidance does not promise an unavailable control. The final
standalone build passed a repeated notification Stop → Back to Nodes → Start
sequence in the same app process, retaining identity, pairing, and the saved
message. However, an immediately submitted connection check failed while
Bluetooth was reconnecting; a later explicit retry succeeded in 139 ms. This
qualifies the restart UI, not first-attempt network readiness. Permissions were
restored, the temporary UI helper was removed, and the app was left running.

### Permission recovery diagnosis

A subsequent temporary diagnostic APK reproduced the failed first check after
Settings Location Deny, app-driven foreground regrant, and immediate Describe.
At 20:59:49.575 on September 8 the request began while the service was running
and Android allowed Bluetooth. The peer connected at 20:59:51.360 and completed
its GATT subscriptions at 20:59:52.299. The request failed after 12.005 seconds
with `EstablishLink(Failed(Timeout))`. A later explicit retry succeeded in
488 ms. The saved pairing remained intact, and the separate background grant
was restored afterward.

This is a Link-establishment timeout, not a permission rejection or Describe
exchange failure. Its timing supports a readiness race but does not prove
which route or interface was selected. Public `route()` reports stored routing
state, not current interface readiness; Android Bluetooth permission readiness
also does not mean that a peer is connected. The next targeted capture should
compare the selected route with `interface_timing_inventory()` immediately
before connecting. No protocol retry or delay was added: a path request emits
on the interfaces available at submission and does not automatically replay
when a peer attaches. Its timeout must also fit the caller's bounded lifetime.
The temporary Rust/Kotlin probes were removed before the delivered build.
The immediate post-Start failure above shows that follow-up must cover ordinary
startup as well as permission recovery; the final build exposed only the generic
Link-stage outcome, not the typed diagnostic captured on the earlier build.

## Remaining work

- Capture the selected route and actual interface readiness during failed early
  requests after startup or Android permission restoration, then qualify its fix.
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
