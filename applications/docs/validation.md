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

At this checkpoint, first-attempt readiness, repeated transitions, system
Location off/on, prolonged peer loss, arbitrary OS process eviction, deep Doze,
and newer Android permission/service behavior remained unqualified. Later
bounded checks are recorded below; they do not complete the full physical
lifecycle matrix. See [Android development](android.md)
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
also does not mean that a peer is connected. The follow-up therefore targeted
the selected route and `interface_timing_inventory()` immediately before
connecting. No protocol retry or delay was added at this diagnostic checkpoint:
a path request emits on the interfaces available at submission and does not
automatically replay when a peer attaches. Its timeout must also fit the caller's
bounded lifetime.
The temporary Rust/Kotlin probes were removed before the delivered build.
The immediate post-Start failure above shows that follow-up must cover ordinary
startup as well as permission recovery; the final build exposed only the generic
Link-stage outcome, not the typed diagnostic captured on the earlier build.

### In-app Stop and bounded connection readiness

The follow-up changes add **Stop node** to Nodes and This device, using the
same service-owned shutdown as the notification action. Stop does not depend on
notification availability. The UI separately reports connection-notification
permission/channel availability and offers an explicit permission request or
Android settings action; neither is a prerequisite for starting the node or
granting Bluetooth access. This is the running-node notification, not message
alerts.

RemoteControl preflight checks the actual route interface's online and
transmit-capable status. This initial checkpoint used a five-second bounded wait
for the current app transports; the later Describe-only follow-up below changes
that cutoff. When necessary it requests the authorized destination's path once
and rechecks interface readiness afterward. A retained route by itself is no
longer sufficient to begin connecting, and the application does not
automatically repeat the requested operation. These checks rely on the status
published by the current Bluetooth/TCP transports, not a universal assumption
about custom interfaces.

Describe now carries one admission-time 20-second deadline across queueing and
network work. Deadline expiry, native caller departure, or priority Stop drops
the owned Describe future and releases active/admitted state. Five focused
paused-time lifecycle tests passed, covering held-future cleanup for each of
those interruptions, expired or abandoned queued commands, queue time deducted
from the remaining network budget, Stop priority over a ready operation, and
preservation of pairing and retained-announcement state. The native library
suite passed 131 tests at that checkpoint, and strict host-test/Android-feature
Clippy passed for the library and tests. These are host checks, not device
qualification or a new JavaScript promise-cancellation contract.

An app-owned connected-target guard queues link closure when its scope ends,
including cancellation. Upstream establishment/identification may already have
issued work before the app receives that handle; cancellation does not prove
immediate engine cleanup at that earlier boundary. AnnounceSelf keeps its
separate retained admission and unknown-outcome behavior.

### Later standalone Android acceptance

Subsequent standalone builds were checked on the same USB-powered Galaxy S9+
without clearing app data. The clean app checkpoint through `86c399456` includes
the readiness, request-lifetime, Stop/notification, and Android keyboard fixes;
the board still used diagnostic firmware. Its gate passed 136 native unit tests,
three real TCP tests, 70 SDK tests, 199 UI tests, Expo Doctor 21/21, strict native
Clippy, and 31 Android JVM tests. ARM64 packaging, API 29 minimum, 16 KB alignment,
and bundled JavaScript checks passed.

- **Controls:** in-app Stop stopped the node; Home/resume left it stopped.
  An earlier standalone build with the same controls also passed the global
  notification-toggle check: with app notifications disabled in Android Settings,
  the UI offered settings guidance while the node remained running, and in-app
  Stop still worked. Restoring notifications removed that guidance. This was
  an Android 10 check, not an Android 13 permission-dialog or individual
  channel-blocking test, and it was not repeated on the later clean checkpoint.
- **First checks:** three consecutive in-app Stop/Start trials returned their
  first authenticated node checks in 146, 144, and 149 ms. Three Location
  revoke/regrant cycles returned first checks in 143, 136, and 180 ms, without
  manual retries; foreground and background grants were restored. System
  Location off showed discovery guidance, and the first check after enabling
  it succeeded. These passes do not include the later radio-toggle failures.
- **Keyboard:** a bottom contact-name field and the compose Message field
  remained visible above the keyboard while typing, moving the cursor, and
  inserting/deleting text. Dismissing the keyboard retained the contact input.
  This resolves the earlier hidden-field reproduction on this phone; no fresh
  iOS or cross-device keyboard qualification is claimed.
- **Messaging and sharing:** a fresh two-way direct LXMF exchange passed over
  the app's Bluetooth connection through the board to the pinned Python peer.
  Both sources were verified, the peer received its delivery proof, and the
  app reported its reply delivered. One **Share node address** action was also
  independently observed as a new live announcement for the exact node,
  excluding path responses; the app's success survived route remount.
- **Process retention:** an explicit force-stop/cold relaunch retained the
  contact's name and pin, both new message directions and the earlier idle-trial
  message, and the same controller/target pairing. This is app-process restart
  evidence, not phone reboot, power-loss, or arbitrary OS-eviction qualification.

Later interactive checks are recorded separately from that clean-build checkpoint:

- **Import picker:** cancelling selection kept Confirm disabled; a malformed
  file was rejected without changing the saved identity. A public 64-byte
  fixture (63 ASCII `A` bytes followed by newline) then produced the exact hash
  independently derived with pinned RNS. Confirm returned `alreadyExists` and
  preserved the existing primary identity. Temporary fixture files were removed.
  This checks picker/preview and overwrite protection, not a pristine app's
  complete import-onboarding flow.
- **Contacts:** a disposable contact was created, pinned and unpinned, with
  each state retained after leaving and reopening its route. Delete removed
  that contact while preserving the actual messaging-peer contact. The later
  clean combined checkpoint below also confirms the deletion after a cold app
  restart; the earlier process-retention pass alone did not cover this deletion.

The connected-Describe cancellation regression also passes over real TCP after
the isolated `codex/fix-local-link-close-schedules` correction. Both core
regressions fail without that correction and pass with it; the core suite passed
1,888 tests with three ignored. This closes the observed stale-schedule defect
after a connected link is closed, not the earlier establishing-link boundary
described above.

A later Galaxy instrumentation run passed both actual-JNI tests, including the
new valid-import test. Public deterministic credentials produced the expected
identity hash; malformed inputs did not create an identity, preview did not
store it, and a different valid credential could not replace it. The imported
hash persisted through two distinct native generations. Both tests used separate
disposable roots in the test APK's sandbox and left production app data alone.
This verifies native import and stop/start persistence, not picker onboarding or
OS process-death durability for a freshly imported identity.

### Repeated Bluetooth recovery: diagnosis and correction

In radio tests following the clean Stop/Start passes, disabling Bluetooth,
stopping a pending check, and starting again reproduced a persistent
GATT-connected state without an MTU callback.
Further radio toggles and a fresh app process did not recover it; an intentional
board reset restored callbacks. Board heartbeats continued during the stall, so
this is not evidence of a whole-board freeze. Temporary raw-callback diagnostics
showed valid owner/epoch/GATT guards and no raw MTU callbacks in eight attempts;
they rule out application callback rejection in that capture, not whether the
Android stack or board caused the stall.

Two separate failures were then reproduced with MTU callbacks working: a changed
listener PSM caused a second runtime restart that interrupted a pending check,
and rapid scan restarts reached Android's scan-rate rejection. App commit
`424848bbe` orders recovery before outbound command admission, avoids duplicate
listener/scan creation, and rejects commands after an incomplete runtime drain.

The next clean standalone build (APK hash prefix `b2c57a1d7f`, through that app
commit) passed the full gate: 136 native unit tests, three real TCP tests, 70 SDK
tests, 203 UI tests, Expo Doctor 21/21, and 42 Android JVM tests. Both actual-JNI
Galaxy instrumentation tests also passed, with the import/isolation limits
described above. These automated results do not establish physical recovery.

The clean board image (hash prefix `11839b1a5e`) included the first isolated
packet-release fix, `5caa97d7d`, which clears unread L2CAP packets at HCI
disconnect. An eight-packet negative control fails without that fix; all 19
library tests passed in each of three configurations with it. The physical
checkpoints on September 8 were:

- **Clean app and board:** seven immediate first checks after radio off/on
  passed in 146, 149, 144, 143, 132, 192, and 189 ms, with one listener/scan per
  recovery. The eighth cycle stalled at 23:10:52 EDT: GATT connected and Android
  accepted the MTU request, but no callback arrived before the eight-second
  deadline. Fresh links remained stalled. The first packet fix is therefore
  insufficient to remove the physical failure.
- **Same app, diagnostic board repeat:** after a board restart, six checks
  passed in 126, 147, 129, 130, 181, and 186 ms. The seventh stalled at 23:19:44
  EDT. The diagnostic image (hash prefix `3965cf513a`) still included only the
  first packet fix. Its trace showed accepted connections/disconnections with
  reference count 1 and 12 credits, but no MTU-processing or packet-pool-error
  markers during the failure. That absence does not identify the fault's owner.

A second commit on the same isolated packet branch, `e95c84a2f`, also releases
queued packets and partial reassembly on terminal channel close and disconnect
confirmation. Three negative-control failures demonstrate those retained-buffer
paths while the original HCI cleanup still passes. Complete library suites pass
23 tests with the eight-packet pool, 22 with optimized reassembly/channel metrics,
and 23 with a one-entry receive queue. The tests retain the old channel handle,
preserve the separate GAP connection, and check closed-reader wakeups. This
second correction was not in either physical checkpoint above. It is now
integrated as `e22d5f59b`, with the following separate device comparison.

#### Integrated cleanup comparison — September 8 diagnostic run

- **Terminal L2CAP cleanup:** diagnostic board image `8bd4d88c1c` completed ten
  MTU recoveries, then stalled on the eleventh radio cycle at 23:45:34 EDT.
  Discovery requests reached the board without an outgoing reply. These ten
  recoveries comprise five immediate app-check passes, one early app-check
  failure despite successful MTU negotiation, and four passes with a two-second
  harness delay. The early failure is a separate timing limit: Bluetooth took
  roughly six seconds to recover while the app's readiness wait ended at five.
  The terminal L2CAP correction alone is insufficient to remove the stall.
- **GATT client queue cleanup:** `61a3741bb` is integrated as `ae411b4197`.
  It releases unconsumed client responses/indications at HCI disconnect. Two
  regressions fail without it: queued packets remain allocated, and a replacement
  connection can receive an old response. All 19 library tests pass with the
  eight-packet pool, a one-entry receive queue, and optimized reassembly/metrics.
- **Latest diagnostic image:** board `c03963e347` includes both L2CAP corrections
  and the GATT cleanup; the phone remains on clean APK `b2c57a1d7f`. Twenty radio
  cycles passed, with checks taking 137–247 ms. Every check was
  submitted after a two-second harness delay, not immediately on radio enable.
  All 425 recorded ATT routing markers showed seven free
  packets after allocating the current RX packet: all eight were available
  before that allocation. No pre-protocol GATT acceptance error was observed.

The latest board trace also captures twelve opcode 29 (`0x1d`, Handle Value
Indication) packets in the server-to-client direction before disconnect, first at
23:49:12.351 EDT. Trouble routes it into `gatt_client`, which the accepted
peripheral session does not consume. The new disconnect cleanup releases that
queue; subsequent ingress retains full pre-RX capacity. This identifies a real
input to the source-proven retention path. Its attribute/value was not captured,
so the indication's purpose is unknown. Old-image accumulation remains inferred
from the ownership regressions and observed stalls, not a captured pool census.

A separate app-only follow-up, `2bd307e4d`, gives Describe readiness the remainder
of its original admission-time 20-second deadline, including time already spent
queued. It does not add another 20 seconds, delay a ready route, or replay the
request. AnnounceSelf retains its five-second readiness wait and existing
unknown-outcome policy; pairing is unchanged. Its 138 native unit tests, three
real TCP integration tests, strict native Clippy, and independent review pass.
Those diagnostic trials used the earlier APK, without this follow-up. The clean
combined checkpoint below tests the new APK and firmware without probes;
diagnostic passes do not retroactively qualify the earlier clean-build failures.

#### Clean combined checkpoint — September 9

The Galaxy S9+/Android 10 ran the standalone, bundled-JavaScript APK through
`2bd307e4d` (SHA256
`4b80f891829ec846516dfbf454cdbbdf49277736c89ab27fb893c90b02967599`).
The board ran clean firmware SHA256
`45272a5fc61faea658f8fe50f8e0357ee6e173c08f11a8ec8a8190a57873485b`,
including both L2CAP corrections and the GATT client cleanup. All temporary
firmware probes were removed; existing phone/board identity and pairing were kept.

- **Gates:** 138 native unit tests, three real TCP tests, 70 SDK tests, 203 UI
  tests, Expo Doctor 21/21, 42 Android JVM tests, strict native Clippy, app/SDK
  typecheck and lint, contracts, web export, and standalone packaging passed.
  Combined Trouble suites passed 25 tests with default/eight-packet settings,
  24 with optimized reassembly/metrics, and 25 with a one-entry receive queue.
  Both actual-JNI Galaxy tests passed again, with zero failures/errors and the
  isolated-storage/import boundaries described above.
- **Radio recovery:** twenty-two physical radio off/on cycles completed with
  successful first checks (displayed durations 135–246 ms) and no repeated
  missing-MTU stall. There was no added two-second harness delay and no check
  was replayed. Some captures in batches d/e/f paused on a UI guard before the
  new result could be proved;
  later result captures and correlated traffic confirmed success. These are
  twenty-two physical successes, not twenty-two automatic script exits of zero.
- **Controls and retention:** Stop followed by Home/return left the node stopped;
  explicit Start's first authenticated check completed in 146 ms. A cold app
  restart showed the disposable contact still deleted and the actual peer still
  pinned. This is app-process evidence, not phone reboot or power-loss coverage.
- **Messaging and sharing:** a fresh direct exchange through the Bluetooth board
  passed in both directions. The Python peer received proof for its message,
  verified the phone's reply, and the app showed that reply delivered in 195 ms.
  One Share node address action reported success in 200 ms and was followed by
  an independently observed live announcement for the exact target, excluding
  path responses. The clean board did not log a command marker: this is UI plus
  timed network observation, not a captured firmware command trace.
- **Offline retry/cancel:** two new messages were admitted with the peer absent
  and became failed connection attempts. After Stop completed, retrying one
  changed that same record to Queued without starting the Android service;
  Cancel changed it to Cancelled. Leaving and reopening the route retained both
  records and their states. After explicit Start and a fresh live messaging
  announcement, a new peer observed neither message during a 60.003-second
  window, including Home/background/return with the same app process and
  foreground service. The cancelled record stayed cancelled and the other stayed
  failed. With a second fresh peer, one explicit retry of the failed message
  produced one verified application delivery callback for its original hash,
  source, title, and content. The app displayed delivery in 140 ms on the same
  card, preserving its original creation time; the cancelled message remained
  cancelled. A further 30.030-second window counted one callback for the retried
  message and none for the cancelled message. This tests manual retry, local
  cancellation, and no automatic resend in bounded observation windows, not
  exactly-once packet transmission or arbitrary process-death recovery.

Evidence is in `/tmp/prns-android-parity-20260908.j4n9sj`:
`final-deadline-verify.log`, `final-clean-native-instrumentation.log`,
`final-clean-radio-a.log` through `final-clean-radio-f.log` and their result
captures, `final-clean-stopped-after-home.txt`, `final-clean-start-check-result.txt`,
`final-clean-contacts-cold.txt`, `final-clean-two-way-peer.log`,
`final-clean-share-complete.txt`, `mailbox-no-resend-peer.log`,
`mailbox-retry-peer.log`, the `mailbox-offline-{queued,cancelled,remount}.txt`
captures, and `mailbox-retry-delivered.txt`. This closes the reproduced recovery
blocker for this bounded clean-device run, not Android-wide or long-idle qualification.

#### Final copy-only build and installation smoke

Commit `ad1ca6007` corrects the pairing and node-control capability descriptions
and adds two catalog tests; it does not change the runtime. Its standalone APK
SHA256 is
`5f8ef4964ccd96420801aba0ca64d5a3a02e89029b8c36a0bdd8573af821779b`.
The build passed in 55 seconds, retaining the same clean board firmware. The
full gate passed again: 138 native unit tests, three real TCP tests, 70 SDK tests,
205 UI tests, Expo Doctor 21/21, and 42 Android JVM tests, plus typecheck, lint,
contracts, and web export.

Installing without clearing data and cold-launching the app produced a successful
first authenticated check (195 ms). The cancelled and retried messages retained
their original IDs, creation times, and Cancelled/Delivered states; the saved
peer remained pinned and the deleted contact stayed absent. Evidence:
`final-copy-verify.log`, `final-copy-android-build.log`,
`final-copy-check-result.txt`, `final-copy-mailbox-loaded.txt`, and
`final-copy-contacts.txt` in the same evidence directory. This is a post-copy
installation/retention smoke check, not a rerun of the twenty-two radio cycles,
two-way messaging, or offline retry/cancel; those remain tied to APK `4b80f89182`.
The app was left running on This device with foreground/background Location
permissions granted. The temporary phone test helper and owned capture files
were removed, and the test peer/log captures were stopped; app data was not cleared.

The Android 10/API 29 implementation is ready for development handoff at the
current iOS app's feature scope, with the qualification limits below. This is
not a release or a fresh qualification of iOS background restoration.

### Publication hold: constrained Nordic firmware

The normal push gate stopped on `t-echo-s140-v7`: its linked firmware exceeds
the configured FLASH region by 1,984 bytes. A matched build of upstream
`1d4d4ba865` with the same Rust 1.98.1 toolchain succeeds, using 622,120 bytes
of firmware image with 472 bytes of headroom. The integrated branch adds
2,448 bytes of `.text` and 136 bytes of `.bss`; `.rodata`, `.data`, the build
configuration, and the memory layout are unchanged. The final linker alignment
accounts for the difference between image occupancy and the reported overflow.
This is an integration regression on a tightly constrained Nordic target, not
an Android runtime failure or an upstream-only build failure.

All 24 host workspaces, root Clippy, and the external-allocation gate passed.
Eleven later selected lanes also passed when run separately: seven additional
Clippy configurations, license-policy parity, both changed-lock dependency
policies, and the unsafe dependency inventory. Browser smoke compiled its Rust
module but could not complete because the local `wasm-bindgen 0.2.126` CLI was
unavailable. These diagnostic continuations do not bypass the failed matrix.
The full firmware matrix and publishing gate have not passed. The equivalent
FLASH limitation was disclosed when the draft PR first opened; that earlier
one-publication hook exception does not authorize another bypass. No flash
layout, feature set, hook, or CI setting was changed to work around this result.
Publication needs a separately reviewed footprint correction or an explicit
exception for updating the draft. Logs: `final-push.log`,
`techo-upstream-baseline.log`, and `techo-source-footprint-review.md` in the
evidence directory above.

## Remaining work

- Resolve the constrained Nordic firmware publication hold above.
- Pristine interactive import onboarding remains separate from the passed
  picker guards and isolated native import test.
- Qualify newer Android permission/service behavior, including the Android 13+
  notification prompt and per-channel notification controls, on suitable hardware
  or an emulator.
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
