# iOS production implementation and hardware evidence

## Status and release boundary

iOS is not production-supported until a macOS contributor returns both:

1. one commit implementing every requirement in this document; and
2. a completed record-only evidence commit in a separate Markdown file whose
   commands, logs, screenshots, and hashes identify the clean implementation
   commit under test. The record commit is discoverable through Git history;
   both SHA fields inside the record name the implementation commit.

The deliverable is a signed direct-install build. App Store and TestFlight work
is outside this gate.

The implementation must preserve the shared `lxmf.delivery` and
`nomadnetwork.node` destinations and the node and Bluetooth identities across
foregrounding, backgrounding, engine restart, process termination, application
relaunch, and operating-system restart.

## Required implementation

### Storage ownership

Swift must obtain the sandboxed Application Support directory with
`FileManager.url(for:in:appropriateFor:create:)`, append a
`PersonalHopspot` directory, create it with intermediate directories enabled,
and pass its filesystem path into Rust. Rust must not derive iOS identity
storage from `HOME`.

Apple defines `URL.applicationSupportDirectory` as the standard application
support location and places it inside the app sandbox on iOS:
<https://developer.apple.com/documentation/foundation/url/applicationsupportdirectory>.

Failure to resolve, create, encode, or configure the directory is a typed
startup failure. It must not fall back to an ephemeral identity.

Rust must keep its runtime snapshots in a private child of that directory. It
must verify the child is writable before publishing readiness; restore the
persisted timebase, cryptographically verified routes, eligible destination
identities, and tunnels at startup; and never replace a corrupt row with
unverified state. Corrupt rows may be refused without blocking valid state or
the core engine.

An accepted announce or route removal must schedule a bounded, debounced
snapshot so learned paths do not depend on receiving a graceful iOS termination
callback. A low-frequency quiet checkpoint must cover other runtime changes.
Explicit shutdown must land one final snapshot before removing the interfaces
whose identifiers and announce records the routes reference.

### Shared host contract

The C header, Rust bridge, and Swift wrapper must use the codes already owned by
`personal-hopspot-core`:

| Contract | Name | Code |
|---|---|---:|
| Input | short press | 0 |
| Input | long press | 1 |
| Action | no platform action | 0 |
| Action | announce | 1 |
| Engine state | stopped | 0 |
| Engine state | starting | 1 |
| Engine state | running | 2 |
| Engine state | failed | 3 |
| Engine failure | none | 0 |
| Engine failure | storage configuration | 1 |
| Engine failure | worker spawn | 2 |
| Engine failure | runtime build | 3 |
| Engine failure | local listener bind | 4 |
| Engine failure | RPC listener bind | 5 |
| Engine failure | startup timeout | 6 |
| Engine failure | worker stopped | 7 |
| Engine failure | shutdown timeout | 8 |

Unknown input values must return the no-action code without changing UI state.
Do not maintain numeric copies in Swift. Export typed C enums and expose
dimension, byte-count, render-cadence, engine-state, and failure accessors from
Rust.

The resulting C surface must separate the engine from the face:

```c
int32_t hopspot_start_engine(const char *storage_directory_utf8);
int32_t hopspot_stop_engine(void);
int32_t hopspot_engine_state(void);
int32_t hopspot_engine_last_failure(void);
HopspotFace *hopspot_init(void);
void hopspot_free(HopspotFace *handle);
int32_t hopspot_post_input(HopspotFace *handle, int32_t code);
void hopspot_announce(void);
void hopspot_render(HopspotFace *handle, uint8_t *ptr, size_t len);
void hopspot_set_battery(HopspotFace *handle, int32_t percent, bool charging);
uint32_t hopspot_panel_width(void);
uint32_t hopspot_panel_height(void);
size_t hopspot_rgba_bytes(void);
uint32_t hopspot_render_interval_millis(void);
```

`hopspot_init` owns only a renderer handle and must not start the engine.
Application lifecycle code owns the restartable engine.

### Engine lifecycle

Use the explicit stopped, starting, running, and failed states. Preserve the
last typed failure until a later successful start clears it.

Startup must:

1. reject missing or conflicting storage configuration;
2. enter starting;
3. detect thread-spawn failure;
4. detect Tokio runtime construction failure;
5. bind required local listeners before publishing running;
6. restore valid persisted runtime state before publishing running;
7. wait no longer than five seconds for readiness; and
8. enter failed with the exact failure code on every unsuccessful path.

Shutdown must flush persisted runtime state, then signal the node, interface
supervisors, Bonjour task, BLE work, USB listener, and local listeners, and wait
no longer than five seconds for the worker to finish. Report stopped only after
that worker has joined. A timeout is failed with `shutdown timeout`; it is not
stopped and a replacement engine must not start while the prior worker remains
alive.

Calling start while running and stop while stopped must be idempotent.
Starting after a completed stop or recoverable failed start must create a fresh
engine around the same persistent identities and destinations.

### Swift lifecycle, rendering, and battery

Remove every WIP marker.

The application delegate or another application-owned service must:

- create and pass the Application Support directory;
- start the engine independently of `HopspotBridge`;
- publish the native engine state and typed last failure to the UI;
- stop and join the engine during explicit application teardown; and
- reconstruct the engine during normal launch and CoreBluetooth restoration
  launch.

Render at the shared 33 ms cadence. Do not use an unconstrained
`TimelineView(.animation)` clock.

Enable UIKit battery monitoring once. Push the initial battery value and update
Rust only from `UIDevice.batteryLevelDidChangeNotification` and
`UIDevice.batteryStateDidChangeNotification`. Remove observers and disable
monitoring when their owner is released. Do not poll battery state per frame.

Create the 1024 by 1024 iOS application icon from
`docs/website/public/assets/favicon.svg`, name the image in
`AppIcon.appiconset/Contents.json`, and fail validation when either the source
PNG or compiled `Assets.car` is absent.

Keep version `0.1.0` and build `1`.

### CoreBluetooth behavior

Retain both `bluetooth-central` and `bluetooth-peripheral` background modes.
Give the central and peripheral managers distinct, stable restoration
identifiers. Recreate the managers with the same identifiers during a
restoration launch and handle both restoration delegate callbacks before
starting unrelated work.

Background support is bounded, not continuous execution. Apple documents that
background scanning is coalesced and slower, background advertising omits or
relocates data, applications may be suspended or terminated, and restoration
is opt-in:
<https://developer.apple.com/library/archive/documentation/NetworkingInternetWeb/Conceptual/CoreBluetooth_concepts/CoreBluetoothBackgroundProcessingForIOSApps/PerformingTasksWhileYourAppIsInTheBackground.html>.

Apple also distinguishes operating-system restoration relaunch from user
force-quit behavior:
<https://developer.apple.com/documentation/technotes/tn3115-bluetooth-state-restoration-app-relaunch-rules/>.

No acceptance statement may claim an indefinitely running background daemon.
The supported claim is that iOS preserves eligible CoreBluetooth intent,
delivers bounded event work, and restores the application when the documented
conditions permit it.

## Simulator gate

The CI job must build the Xcode application for one concrete, available iPhone
or iPad simulator. A generic simulator destination or macOS-host Rust build is
not sufficient.

Run from the repository root:

```bash
rustup target add aarch64-apple-ios-sim
SIMULATOR_ID="$(
  xcrun simctl list devices available -j |
    python3 -c 'import json,sys; devices=json.load(sys.stdin)["devices"]; print(next(device["udid"] for runtime in devices.values() for device in runtime if device["name"].startswith(("iPhone", "iPad"))))'
)"
test -n "${SIMULATOR_ID}"
xcrun simctl boot "${SIMULATOR_ID}" 2>/dev/null || true
xcrun simctl bootstatus "${SIMULATOR_ID}" -b
xcodebuild \
  -project personal-hopspot/mobile/ios/app/PersonalHopspot.xcodeproj \
  -scheme PersonalHopspot \
  -configuration Debug \
  -destination "id=${SIMULATOR_ID}" \
  -derivedDataPath /tmp/personal-hopspot-ios-derived \
  build
APP=/tmp/personal-hopspot-ios-derived/Build/Products/Debug-iphonesimulator/PersonalHopspot.app
test -f personal-hopspot/mobile/ios/app/PersonalHopspot/Assets.xcassets/AppIcon.appiconset/AppIcon.png
test -f "${APP}/Assets.car"
xcrun simctl install "${SIMULATOR_ID}" "${APP}"
LAUNCH_OUTPUT="$(xcrun simctl launch --terminate-running-process "${SIMULATOR_ID}" com.personal.hopspot)"
printf '%s\n' "${LAUNCH_OUTPUT}"
LAUNCH_PID="$(printf '%s\n' "${LAUNCH_OUTPUT}" | awk -F': ' '/com\.personal\.hopspot:/ {print $2}')"
[[ "${LAUNCH_PID}" =~ ^[0-9]+$ ]]
sleep 5
xcrun simctl spawn "${SIMULATOR_ID}" launchctl print user/501 > /tmp/personal-hopspot-launchctl.txt
LAUNCHCTL_LINE="$(grep 'UIKitApplication:com\.personal\.hopspot' /tmp/personal-hopspot-launchctl.txt)"
test "$(printf '%s\n' "${LAUNCHCTL_LINE}" | awk '{print $1}')" = "${LAUNCH_PID}"
xcrun simctl io "${SIMULATOR_ID}" screenshot /tmp/personal-hopspot-ios.png
test -s /tmp/personal-hopspot-ios.png
```

Expected output includes `** BUILD SUCCEEDED **`, a launch line containing
`com.personal.hopspot` and a numeric process identifier, a matching
`UIKitApplication` entry from the simulator's per-user launchd domain after
five seconds, and a nonempty screenshot. Set `LAUNCH_PID` from the numeric
identifier returned by `simctl launch`.

The gate fails on any nonzero command, missing icon, missing `Assets.car`,
missing process, empty screenshot, startup state other than running, nonzero
last-failure code, or crash report for `PersonalHopspot` during the run.

Add the simulator script to `validation/platforms/`, register it in
`validation/manifest.toml`, and invoke it from the macOS CI job.

## Physical hardware matrix

Test one supported iPhone and one supported iPad. Record the exact model,
operating-system build, Xcode build, Rust toolchain, commit, signing identity,
and binary SHA-256 for each row. Simulator results do not substitute for any
row because Apple explicitly notes that simulators do not reproduce every
hardware feature:
<https://developer.apple.com/documentation/Xcode/running-your-app-on-simulated-or-physical-devices>.

| Scenario | iPhone | iPad | Required evidence |
|---|---|---|---|
| Fresh install |  |  | launch log, running state, no failure |
| Initial Bluetooth denial |  |  | core engine running; BLE unavailable without crash |
| Initial local-network denial |  |  | core engine running; LAN unavailable without crash |
| Later Bluetooth grant |  |  | BLE becomes usable without reinstall |
| Later local-network grant |  |  | Bonjour/LAN becomes usable without reinstall |
| Foreground to background |  |  | lifecycle log and documented bounded behavior |
| Background to foreground |  |  | same engine identity and resumed rendering/input |
| Explicit native stop/start |  |  | joined stop, fresh worker, same identity |
| Termination and relaunch |  |  | same identity and destinations |
| Learned-route relaunch |  |  | accepted route flushed, process replaced, same verified route restored |
| Operating-system restart |  |  | same identity and destinations |
| CoreBluetooth central restoration |  |  | restoration launch and resumed central state |
| CoreBluetooth peripheral restoration |  |  | restoration launch and resumed peripheral state |
| BLE central traffic |  |  | bidirectional frames and byte counters |
| Bluetooth LE peripheral traffic |  |  | bidirectional frames and byte counters |
| Bonjour discovery |  |  | peer discovery with address and port |
| LAN traffic |  |  | bidirectional frames and byte counters |
| usbmux USB Auto |  |  | physical cable, peer formation, bidirectional frames |
| Short press |  |  | visible focus transition |
| Long press |  |  | visible menu transition |
| Announce action |  |  | both shared destinations announced |
| Four-hour sustained run |  |  | no crash, bounded memory, final counters and battery |

For denial tests, remove the app first so the permission prompt is genuinely
initial. Denial must affect only the corresponding transport. It must not
prevent identity loading, engine startup, rendering, input, USB, or other
permitted transports.

Separately delay each initial permission-sheet response beyond the backend's
readiness timeout. BLE and Bonjour may report transient failed attempts, but
each must retry and become usable in that same process after the sheet is
approved. A relaunch does not prove recovery from a human-scale prompt delay.

For the later Settings-grant rows, do not reinstall or manually terminate the
app. iPadOS or iOS may replace the process when a privacy toggle changes. If it
does, record the operating-system termination and subsequent launch, then prove
identity continuity and transport recovery from the same installation.

For restoration tests, establish a pending or active Bluetooth operation, move
the app to the background, and use an operating-system termination/restoration
scenario consistent with TN3115. Record user force-quit separately; it is not a
valid restoration success case.

For the sustained run, sample process memory and battery at start, hourly, and
finish. Fail on crash, native worker loss, unbounded memory growth, thermal
shutdown, identity change, listener loss that does not recover, or a transport
remaining falsely online after permission revocation.

During stabilization before the first supported iOS release, the product owner
may authorize focused requalification after an implementation change. The
evidence must name the earlier implementation SHA, enumerate every
carried-forward row, explain why each row is materially unaffected, and run
every row touched by the change against the new clean SHA. Permission,
lifecycle, transport, or persistence evidence may not be carried forward across
a change to the corresponding subsystem.

## Required commands on each physical device

Replace the placeholders with the concrete device identifier and evidence
directory:

```bash
DEVICE_ID=<device-udid>
EVIDENCE_DIR=<absolute-evidence-directory>
mkdir -p "${EVIDENCE_DIR}"
xcodebuild \
  -project personal-hopspot/mobile/ios/app/PersonalHopspot.xcodeproj \
  -scheme PersonalHopspot \
  -configuration Release \
  -destination "id=${DEVICE_ID}" \
  -derivedDataPath /tmp/personal-hopspot-ios-device \
  build | tee "${EVIDENCE_DIR}/xcodebuild.log"
APP=/tmp/personal-hopspot-ios-device/Build/Products/Release-iphoneos/PersonalHopspot.app
shasum -a 256 "${APP}/PersonalHopspot" | tee "${EVIDENCE_DIR}/binary.sha256"
xcrun devicectl device install app --device "${DEVICE_ID}" "${APP}" |
  tee "${EVIDENCE_DIR}/install.log"
xcrun devicectl device process launch --device "${DEVICE_ID}" com.personal.hopspot |
  tee "${EVIDENCE_DIR}/launch.log"
xcrun devicectl device info details --device "${DEVICE_ID}" |
  tee "${EVIDENCE_DIR}/device.txt"
xcodebuild -version | tee "${EVIDENCE_DIR}/xcode.txt"
rustc --version --verbose | tee "${EVIDENCE_DIR}/rustc.txt"
git rev-parse HEAD | tee "${EVIDENCE_DIR}/commit.txt"
```

Capture unified logs for every scenario with Xcode’s device console or
`devicectl`, preserving timestamps. Each log must contain the engine state,
typed failure code, abbreviated node identity hash, destination hashes,
transport transition, and byte counters relevant to that scenario.

## Failure criteria

The entire iOS gate fails if any of the following is true:

- either SHA field in the committed evidence record differs from the clean
  implementation commit that produced the evidence;
- the worktree or generated application is dirty or unaccounted for;
- identity storage derives from `HOME` or falls back to ephemeral material;
- accepted learned routes are not flushed and restored across process
  replacement;
- corrupt persisted rows can block valid state, bypass verification, or crash
  startup;
- face creation starts or owns the engine;
- startup or shutdown can wait without a bound;
- stopped is reported before the worker and native tasks finish;
- a listener bind, runtime build, or worker-spawn failure is untyped;
- an unknown input becomes a short press;
- Swift duplicates framebuffer or cadence facts without deriving them from Rust;
- rendering exceeds the shared cadence or battery is polled per frame;
- the icon, compiled asset catalog, privacy strings, or notices are absent;
- simulator build, install, launch, liveness, or screenshot evidence fails;
- permission denial prevents the core engine from running;
- either physical device row is missing;
- any required transport lacks bidirectional traffic evidence;
- restoration is claimed from a user force-quit test;
- background behavior is described as unlimited execution;
- the four-hour run crashes, leaks without bound, changes identity, or leaves
  stale transport state; or
- any command output, log, screenshot, or SHA-256 referenced by the evidence is
  missing.

## Evidence record template

Copy this section into a new evidence file. Do not edit this procedure to turn
a failed item into a pass.

```markdown
# iOS production evidence

Implementation commit:
Evidence commit: <!-- repeat the clean implementation SHA under test; the containing record-only commit is discoverable in Git history -->
Contributor:
Date in UTC:
Xcode:
Rust:
Signing identity:

## Artifacts

| Artifact | Path | SHA-256 |
|---|---|---|
| iPhone application binary |  |  |
| iPad application binary |  |  |
| Simulator screenshot |  |  |
| iPhone log bundle |  |  |
| iPad log bundle |  |  |

## Devices

| Role | Model | UDID suffix | OS version and build |
|---|---|---|---|
| iPhone |  |  |  |
| iPad |  |  |  |

## Simulator gate

Concrete simulator:
Build result:
Install result:
Launch PID:
Five-second liveness:
Engine state:
Last failure:
Screenshot:
Result:

## Physical matrix

| Scenario | iPhone result and evidence | iPad result and evidence |
|---|---|---|
| Fresh install |  |  |
| Initial Bluetooth denial |  |  |
| Initial local-network denial |  |  |
| Later Bluetooth grant |  |  |
| Later local-network grant |  |  |
| Foreground to background |  |  |
| Background to foreground |  |  |
| Explicit native stop/start |  |  |
| Termination and relaunch |  |  |
| Operating-system restart |  |  |
| CoreBluetooth central restoration |  |  |
| CoreBluetooth peripheral restoration |  |  |
| BLE central traffic |  |  |
| Bluetooth LE peripheral traffic |  |  |
| Bonjour discovery |  |  |
| LAN traffic |  |  |
| usbmux USB Auto |  |  |
| Short press |  |  |
| Long press |  |  |
| Announce action |  |  |
| Four-hour sustained run |  |  |

## Identity continuity

| Checkpoint | Node identity hash | BLE identity | Delivery destination | Node-page destination |
|---|---|---|---|---|
| Fresh install |  |  |  |  |
| Native restart |  |  |  |  |
| Process relaunch |  |  |  |  |
| OS restart |  |  |  |  |

## Sustained operation

| Device | Checkpoint | Memory | Battery | Thermal state | RX bytes | TX bytes |
|---|---|---:|---:|---|---:|---:|
| iPhone | Start |  |  |  |  |  |
| iPhone | Hour 1 |  |  |  |  |  |
| iPhone | Hour 2 |  |  |  |  |  |
| iPhone | Hour 3 |  |  |  |  |  |
| iPhone | Hour 4 |  |  |  |  |  |
| iPad | Start |  |  |  |  |  |
| iPad | Hour 1 |  |  |  |  |  |
| iPad | Hour 2 |  |  |  |  |  |
| iPad | Hour 3 |  |  |  |  |  |
| iPad | Hour 4 |  |  |  |  |  |

## Failures and deviations

None, or list every failure without reclassifying it.

## Acceptance

- [ ] Every required implementation item is present.
- [ ] Simulator gate passed.
- [ ] iPhone matrix passed.
- [ ] iPad matrix passed.
- [ ] Identity remained stable.
- [ ] Background claims match Apple’s bounded model.
- [ ] All referenced evidence exists and is hashed.

Final result: PASS or FAIL
```
