# Android production hardware gate

## Status and release boundary

Android is not production-supported until both hardware columns in this
document have completed evidence for the same commit:

| Matrix | Required device |
|---|---|
| Legacy projector | API 19, 32-bit ARMv7 |
| Modern mobile | API 33 or newer, 64-bit ARM64 |

The deliverable is a signed direct-install APK. Play Store work is outside this
gate.

Wi-Fi Direct is an experimental transport and is disabled in every Android
build unless the operator explicitly opts in at build time. It is outside this
production gate and cannot contribute a passing or failing production result.
Its current evidence and laboratory boundary are recorded in
[`wifi-direct-experimental.md`](wifi-direct-experimental.md).

The application must retain version `0.1.0`, build `1`, the shared
`lxmf.delivery` and `nomadnetwork.node` destinations, and persistent node and
Bluetooth identities across service restart, process recreation, application
relaunch, and device restart. Its routing table, known destination identities,
tunnels, monotonic timeline high-water, and local destination ratchets must
also survive every graceful engine restart. State learned before the most
recent 30-second persistence boundary must survive ungraceful process death.

## Build gate

Run from the repository root on Linux:

```bash
python3 validation/run.py verify
cargo test --locked --manifest-path personal-hopspot/core/Cargo.toml
cargo clippy --locked --manifest-path personal-hopspot/core/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path personal-hopspot/mobile/android/rust/Cargo.toml
cargo clippy --locked --manifest-path personal-hopspot/mobile/android/rust/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path prns-ffi/Cargo.toml
cargo clippy --locked --manifest-path prns-ffi/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml --features wifi-direct wifi_direct::runtime::tests::toggle_sleep_and_wake_change_the_aggregate_state
bash validation/platforms/android-service-smoke.sh
```

Expected output includes passing Rust tests and Clippy, both Android release
library builds, successful dependency verification, lint, unit tests, APK
assembly, and `ANDROID_SERVICE_SMOKE_OK`.

The gate fails on any nonzero command, dependency drift, lint error, missing
release library, missing launcher icon, backup-enabled manifest, permission
contract mismatch, version mismatch, missing foreground service contract, or
missing third-party notices. It also fails if the default APK enables the
experimental Wi-Fi Direct platform link.

Record:

```bash
git rev-parse HEAD
rustc -Vv
java -version
sha256sum personal-hopspot/mobile/android/app/build/outputs/apk/release/app-release.apk
${ANDROID_HOME}/build-tools/34.0.0/apksigner verify --verbose --print-certs personal-hopspot/mobile/android/app/build/outputs/apk/release/app-release.apk
```

Do not install an unsigned or debug APK for production evidence.

Set the four release-signing values and build the production artifact:

```bash
export PRNS_ANDROID_KEYSTORE=/absolute/path/to/prns-release.jks
export PRNS_ANDROID_KEYSTORE_PASSWORD='<from the release credential store>'
export PRNS_ANDROID_KEY_ALIAS='<release alias>'
export PRNS_ANDROID_KEY_PASSWORD='<from the release credential store>'
(
  cd personal-hopspot/mobile/android
  ./gradlew --no-daemon :app:assembleProduction
)
```

`assembleProduction` fails when any credential is absent or the keystore path
is not a file. Do not commit the keystore, passwords, or generated APK.

## Device inventory

For each device, capture:

```bash
adb -s "${ANDROID_SERIAL}" shell getprop ro.product.manufacturer
adb -s "${ANDROID_SERIAL}" shell getprop ro.product.model
adb -s "${ANDROID_SERIAL}" shell getprop ro.product.device
adb -s "${ANDROID_SERIAL}" shell getprop ro.build.version.sdk
adb -s "${ANDROID_SERIAL}" shell getprop ro.build.version.release
adb -s "${ANDROID_SERIAL}" shell getprop ro.build.fingerprint
adb -s "${ANDROID_SERIAL}" shell getprop ro.product.cpu.abilist
adb -s "${ANDROID_SERIAL}" shell pm list features
```

The legacy device must report API 19 and an ARMv7 ABI. The modern device must
report API 33 or newer and an ARM64 ABI. Record unavailable optional hardware
as unavailable before testing; absence is not a pass for a feature the device
reports.

## Clean installation

Save any existing user data before this step. Then install the signed APK:

```bash
adb -s "${ANDROID_SERIAL}" uninstall org.personal.hopspot
adb -s "${ANDROID_SERIAL}" install personal-hopspot/mobile/android/app/build/outputs/apk/release/app-release.apk
adb -s "${ANDROID_SERIAL}" shell am start -W -n org.personal.hopspot/.MainActivity
adb -s "${ANDROID_SERIAL}" shell dumpsys package org.personal.hopspot
adb -s "${ANDROID_SERIAL}" shell dumpsys activity services org.personal.hopspot
adb -s "${ANDROID_SERIAL}" logcat -d -v threadtime
```

Expected results:

- installation and launch succeed without a native loader or verifier error;
- the packaged version is `0.1.0` with version code `1`;
- the launcher displays the Prns icon;
- `PrnsService` becomes a foreground service;
- engine state reaches `running` with failure `none`;
- the local listener uses port `37428` and RPC uses port `37429`; and
- logs contain no fatal exception, ANR, native panic, or crash.

## Modern API 33+ ARM64 matrix

### Permission denial and recovery

Begin from a clean install. Deny notification, nearby-device, and Bluetooth
requests. Query package grants and service state:

API 33 and newer use `NEARBY_WIFI_DEVICES`; API 23 through 32 retain location
permission for the Wi-Fi APIs, following Android's
[nearby Wi-Fi permission contract](https://developer.android.com/develop/connectivity/wifi/wifi-permissions).

```bash
adb -s "${ANDROID_SERIAL}" shell dumpsys package org.personal.hopspot
adb -s "${ANDROID_SERIAL}" shell dumpsys activity services org.personal.hopspot
adb -s "${ANDROID_SERIAL}" logcat -d -v threadtime
```

The core engine must remain running. Wi-Fi Aware must report no permission, BLE
must remain inactive, the Wi-Fi Direct lab interface must report that the
experimental transport is disabled, and Wi-Fi Auto, local listeners,
rendering, and input must remain usable.

Run the bounded permission sequence:

```bash
ANDROID_SERIAL="${ANDROID_SERIAL}" bash validation/platforms/android-permission-recovery-smoke.sh
```

Expected output ends with `ANDROID_PERMISSION_RECOVERY_SMOKE_OK`.

Grant only nearby Wi-Fi:

```bash
adb -s "${ANDROID_SERIAL}" shell pm grant org.personal.hopspot android.permission.NEARBY_WIFI_DEVICES
adb -s "${ANDROID_SERIAL}" shell am force-stop org.personal.hopspot
adb -s "${ANDROID_SERIAL}" shell am start -W -n org.personal.hopspot/.MainActivity
```

Supported Wi-Fi Aware behavior must recover while BLE remains inactive. The
Wi-Fi Direct lab interface must remain disabled. Then grant Bluetooth:

```bash
adb -s "${ANDROID_SERIAL}" shell pm grant org.personal.hopspot android.permission.BLUETOOTH_SCAN
adb -s "${ANDROID_SERIAL}" shell pm grant org.personal.hopspot android.permission.BLUETOOTH_ADVERTISE
adb -s "${ANDROID_SERIAL}" shell pm grant org.personal.hopspot android.permission.BLUETOOTH_CONNECT
adb -s "${ANDROID_SERIAL}" shell am force-stop org.personal.hopspot
adb -s "${ANDROID_SERIAL}" shell am start -W -n org.personal.hopspot/.MainActivity
```

BLE must recover without clearing application data or changing identities.

### Foreground, background, stop, and restart

Run the automated foreground probe:

```bash
ANDROID_SERIAL="${ANDROID_SERIAL}" bash validation/platforms/android-runtime-smoke.sh
ANDROID_SERIAL="${ANDROID_SERIAL}" bash validation/platforms/android-sticky-recreation-smoke.sh
```

Then exercise explicit service shutdown and restart:

```bash
adb -s "${ANDROID_SERIAL}" shell am instrument -w org.personal.hopspot.test/org.personal.hopspot.PrnsRuntimeProbe
```

Expected results:

- backgrounding does not stop the foreground service;
- explicit stop removes the notification and service only after native work
  has stopped;
- restart returns to `running` within five seconds;
- injected process death recreates the sticky foreground service within twenty
  seconds under a new PID;
- no prior listener remains bound during restart; and
- the RPC key and both identities remain unchanged.

The probe prints domain-separated SHA-256 fingerprints named
`rpc_identity_sha256`, `node_identity_sha256`,
`bluetooth_identity_sha256`, `delivery_destination_sha256`, and
`node_page_destination_sha256`. Record those values after each lifecycle
transition. A raw RPC key or Bluetooth identity is not release evidence and
must not be copied into the evidence record.

### Runtime-state durability

The runtime writes sealed, checksummed state beneath the app-private files
directory. Each region lands through a synced staging file and atomic rename.
The service reports how many routing-table rows, known destination identities,
tunnels, and ratcheted destinations restored; how many rows were refused or
dropped; and how many state flushes landed in the current engine generation.

The ordinary runtime probe must report:

- `restored_ratchet_count` of at least one after its service restart;
- `refused_restore_count=0`;
- `successful_flush_count` of at least one; and
- nonnegative route, destination-identity, tunnel, and dropped-row counts.

Start a second known-good RNS node on a production transport and keep it
announcing until Hopspot learns at least one route. Then run:

```bash
ANDROID_SERIAL="${ANDROID_SERIAL}" bash validation/platforms/android-routing-persistence-smoke.sh
```

The bounded probe waits for a nonempty route table, waits for a periodic flush
strictly newer than the observation, kills only the Hopspot application
process, waits for sticky service recreation under a new PID, and requires a
nonzero restored-route count, known-destination count, and ratchet count with
zero refused rows. The aggregate live-interface route count may remain zero
until the departed transport interface reconnects and reclaims the restored
row. Expected output ends with `ANDROID_ROUTING_PERSISTENCE_SMOKE_OK`.

The gate fails if the engine reports running before its initial durable flush,
if a periodic or shutdown flush fails, if the service reports stopped before
the final flush completes, if any stored row is refused without an explained
corruption-recovery exercise, or if the learned route is not accepted during
the bounded crash/recreation restore.

### Process and device recreation

Record the RPC key and identity fingerprints, then run:

```bash
adb -s "${ANDROID_SERIAL}" shell am force-stop org.personal.hopspot
adb -s "${ANDROID_SERIAL}" shell am start -W -n org.personal.hopspot/.MainActivity
adb -s "${ANDROID_SERIAL}" reboot
adb -s "${ANDROID_SERIAL}" wait-for-device
adb -s "${ANDROID_SERIAL}" shell am start -W -n org.personal.hopspot/.MainActivity
```

The application must return to `running`; the RPC key, node identity,
Bluetooth identity, and destination hashes must match the original values.

### Rendering and input

Capture video showing the initial screen, short press, long press, menu
navigation, announce action, backgrounding, and return:

```bash
adb -s "${ANDROID_SERIAL}" shell screenrecord /sdcard/prns-render-input.mp4
adb -s "${ANDROID_SERIAL}" pull /sdcard/prns-render-input.mp4
```

The renderer must update at the shared 33 ms cadence without stretching,
corruption, stalled input, or crash.

### Transport proofs

Use a second known-good Prns node and record peer logs plus packet counts for
each supported transport.

| Transport | Required proof |
|---|---|
| BLE Auto | central and peripheral discovery, bidirectional payloads, disconnect, and reconnect |
| Wi-Fi Auto | Bonjour/LAN discovery and bidirectional payloads |
| Wi-Fi Aware | discovery, data-path formation, bidirectional payloads, teardown, and recovery when the device reports the feature |
| USB host | attach, permission grant or denial, bidirectional payloads, detach, and reattach |
| USB AOA | accessory negotiation, bidirectional payloads, detach, and reattach |
| Shared instance | a same-signature client binds, obtains running status and RPC credentials, and exchanges traffic |

Confirm that `WiFi Direct Lab` remains disconnected with the experimental
disabled reason throughout the production run. Do not opt in to the transport
for production evidence.

For unsupported optional transports, attach the `pm list features` evidence
and mark the row `not exposed by device`. Do not mark it passed.

### Sustained operation

Run for at least eight hours with BLE Auto and Wi-Fi Auto enabled and at least
one continuous traffic stream:

```bash
adb -s "${ANDROID_SERIAL}" shell dumpsys batterystats --reset
adb -s "${ANDROID_SERIAL}" logcat -c
adb -s "${ANDROID_SERIAL}" logcat -v threadtime
```

At the end, capture service state, memory, battery, sockets, and logs:

```bash
adb -s "${ANDROID_SERIAL}" shell dumpsys activity services org.personal.hopspot
adb -s "${ANDROID_SERIAL}" shell dumpsys meminfo org.personal.hopspot
adb -s "${ANDROID_SERIAL}" shell dumpsys batterystats org.personal.hopspot
adb -s "${ANDROID_SERIAL}" shell cat /proc/net/tcp
adb -s "${ANDROID_SERIAL}" logcat -d -v threadtime
```

Fail on crash, ANR, native panic, stuck starting state, unexpected failed state,
unbounded memory growth, dead listener, unrecovered transport, or identity
change.

## API 19 ARMv7 matrix

Use the ARMv7 release library built with native API floor 21 and the intentional
projector compatibility path. The Kotlin and Java bytecode must target Java 8.

Run clean install and launch, then capture:

```bash
adb -s "${ANDROID_SERIAL}" shell am start -W -n org.personal.hopspot/.MainActivity
adb -s "${ANDROID_SERIAL}" shell dumpsys activity services org.personal.hopspot
adb -s "${ANDROID_SERIAL}" shell dumpsys meminfo org.personal.hopspot
adb -s "${ANDROID_SERIAL}" logcat -d -v threadtime
```

Required scenarios:

| Scenario | Required result |
|---|---|
| Install and native load | APK installs; ARMv7 library loads without unresolved symbol or verifier failure |
| Launch | renderer appears and engine reaches running |
| Input | short and long presses produce the expected navigation and no unknown-input fallback |
| Background and return | foreground service remains valid and rendering resumes |
| Explicit stop and restart | native worker joins, listeners release, and restart reaches running |
| Process recreation | application relaunches without changing identity |
| Device restart | application launch retains identity and destinations |
| Runtime-state persistence | route, destination, tunnel, timeline, and ratchet snapshots restore without refused rows |
| Wi-Fi Auto | LAN discovery and bidirectional payloads pass |
| Wi-Fi Direct lab | remains disabled; the transport is outside the production gate |
| USB host or AOA | every mode exposed by the device passes attach, traffic, detach, and reattach |
| BLE | record unavailable below the supported Android BLE host floor unless this build explicitly exposes it |
| Wi-Fi Aware | record unavailable because Android API 19 does not expose Wi-Fi Aware |
| Sustained operation | four hours without crash, ANR, dead listener, identity change, or unbounded memory growth |

The API 19 run fails if any API 21-or-newer framework class is loaded on the
legacy path before its version guard.

## Evidence record

Create a separate file named
`validation/evidence/android-production-<commit>.md`. Include all fields below.

```text
# Android production evidence

Commit:
Source tree clean:
APK path:
APK SHA-256:
Signer certificate SHA-256:
Version name:
Version code:
Rust toolchain:
JDK:
Android SDK:
Android NDK:
Build gate log:

## API 19 ARMv7 device

Manufacturer:
Model:
Device:
API/release:
Build fingerprint:
ABI list:
Feature list:
Start timestamp:
End timestamp:

| Scenario | Result | Evidence paths and timestamps |
|---|---|---|
| Clean install and launch |  |  |
| Render and input |  |  |
| Background and return |  |  |
| Explicit stop and restart |  |  |
| Process recreation |  |  |
| Device restart |  |  |
| Identity and destinations |  |  |
| Runtime-state persistence |  |  |
| Wi-Fi Auto |  |  |
| Wi-Fi Direct lab disabled |  |  |
| USB host |  |  |
| USB AOA |  |  |
| Exposed additional transport |  |  |
| Sustained operation |  |  |

## API 33+ ARM64 device

Manufacturer:
Model:
Device:
API/release:
Build fingerprint:
ABI list:
Feature list:
Start timestamp:
End timestamp:

| Scenario | Result | Evidence paths and timestamps |
|---|---|---|
| Clean install and launch |  |  |
| Initial full denial |  |  |
| Nearby-only partial grant |  |  |
| Later Bluetooth grant |  |  |
| Foreground and background |  |  |
| Explicit stop and restart |  |  |
| Sticky service recreation |  |  |
| Process recreation |  |  |
| Device restart |  |  |
| Identity and destinations |  |  |
| Runtime-state persistence |  |  |
| Render and input |  |  |
| BLE Auto |  |  |
| Wi-Fi Auto |  |  |
| Wi-Fi Direct lab disabled |  |  |
| Wi-Fi Aware |  |  |
| USB host |  |  |
| USB AOA |  |  |
| Shared-instance binding |  |  |
| Sustained operation |  |  |

## Failures and deviations

None, or list each failure with reproduction steps and disposition.

## Sign-off

Operator:
Date:
API 19 ARMv7 production gate:
API 33+ ARM64 production gate:
Android production-supported:
```

Every `pass` entry must link to raw logs, screenshots or video where relevant,
and exact command output. Redact private keys and payload content, but retain
stable one-way identity fingerprints so persistence can be compared.

## Acceptance

Android may be marked production-supported only when:

- the build gate passes at the evidenced commit;
- both required physical-device matrices pass;
- all device-exposed production transports pass;
- the Wi-Fi Direct lab transport remains disabled;
- permission denial and later recovery leave the core engine running;
- service stop, restart, sticky recreation, and native lifecycle agree;
- identities and destinations remain stable;
- routing, known-destination, tunnel, timeline, and ratchet state pass the
  graceful and crash-recreation persistence proofs;
- the direct-install APK is signed and its hash is recorded; and
- no open failure or unexplained deviation remains.
