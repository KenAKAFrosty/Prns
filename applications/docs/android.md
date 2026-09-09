# Android development

The Android application targets Android 10 (API 29) and newer. This is the
minimum version used by the upstream Bluetooth L2CAP transport, not the compile
or target SDK. The Expo/React Native toolchain currently compiles and targets
API 36. Initial physical checks use a Galaxy S9+ running Android 10.

## Ownership

The Kotlin Expo module translates values and platform lifecycle into the same
application-owned Rust runtime used by iOS. It does not run Personal Hopspot's
separate engine. A non-exported foreground service owns the platform adapters;
React reloads and Activity destruction do not stop the node. The service has an
ongoing connection notification when Android permits it. Both its Stop action
and the in-app **Stop node** control use the same service-owned shutdown.
Android may still terminate the process. A sticky service restart reopens the
stored identity and the last successful start configuration; a user Stop clears
that restart intent.

After notification Stop, ordinary navigation or returning to the app leaves the
node stopped. **Start node** on Nodes or This device explicitly starts it again
through the existing runtime owner. The stopped inventory says **Paired nodes
unavailable**, rather than implying that saved pairings were deleted. Pairings
return when startup completes; no force-stop or app relaunch is required.

Identity, grants, contacts, and messages remain Rust-owned under the application's
private `noBackupFilesDir/prns/development` directory. These are disposable
development records, not a secure-storage or migration guarantee.

Bluetooth uses the public upstream Android backend and an attributed adaptation
of its Kotlin transport glue. Android permissions and discovery are independent
of Apple AccessorySetupKit and of RemoteControl invitation/grant pairing. The
node and TCP interface can start without granting Bluetooth permission.

## Node controls

On **Nodes** or **This device**, select **Stop node** to stop this device's
connections without deleting its identity, pairings, contacts, or messages.
Wait for stopping to finish before selecting **Start node**. Leaving the page
or returning to the app does not restart it. The in-app Stop control remains
available independently of notification permission.

The running-node notification also provides **Stop** outside the app. If it is
not enabled, **Allow node notification** requests notification permission on
Android 13 and newer. When Android has blocked notifications for the app or its
connection channel, **Open notification settings** opens the app's Android
settings instead; enable the relevant notification there. Returning to the app
refreshes the reported setting. This permission is separate from Bluetooth and
background discovery, and does not enable message alerts. It is not required to
start the node or use the in-app Stop control.

## Build

Install the Android SDK, JDK 21, the repository Rust toolchain, and the Rust
`aarch64-linux-android` target. Set `ANDROID_HOME` to the SDK directory. Install
the NDK revision and `cargo-ndk` version recorded in
[`vendor/ubrn/source-lock.json`](../vendor/ubrn/source-lock.json); the build helper
rejects a different NDK. Follow the [workspace setup](../README.md#setup) to
install the JavaScript dependencies and build the core contract. The
[generation guide](../tools/generated-bindings/README.md) explains target
selection and build caches.

From `applications/`:

```sh
npm run native:android:client
npm run native:android:standalone
```

The first command builds a development client that uses Metro. The second bundles
JavaScript into a standalone development APK. Both use the development application
identifier and local debug signing; neither is a production distribution build.
Generated Android projects and native build outputs are ignored. The tracked
Expo config plugin, module sources, and build scripts are the source of truth.

Builds select `arm64-v8a` by default to limit disk and build time. Set
`PRNS_ANDROID_ABIS=x86_64` for an Intel emulator, or a comma-separated list for both
supported ABIs. Rust and APK packaging use 16 KB page alignment. Native sources
are rebuilt through Cargo's incremental graph on every Android native build.

Install the resulting APK using an explicitly selected device. For the Metro
client, forward that device's Metro port over USB; do not assume another phone's
connection settings. The standalone APK does not need a Metro connection.

## Permissions and limits

On Android 10–11, Bluetooth discovery requires location permission and enabled
Location services. Background discovery requires a separate background-location
grant. The UI requests these separately and only after an explicit user action.
On Android 12+, discovery uses Nearby devices permissions. The service does not
make these grants implicit or bypass Android's foreground-service launch limits.

The runtime uses a connected-device service for communication with external
Bluetooth/network peers. This is not a blanket promise that every future
interface can run indefinitely in the background. Wi-Fi Auto, USB, Wi-Fi Aware,
and Wi-Fi Direct are not enabled by this initial Android composition.

The current upstream backend reads the local Bluetooth listener's PSM at
interface startup. If Android replaces that listener with a different PSM,
the app explicitly restarts its native runtime with the same saved identity.
This also interrupts any TCP connections. Live capability refresh belongs in
the shared Bluetooth backend; this restart is a temporary recovery boundary,
not a claim of seamless radio-toggle recovery.

### Current device checkpoint

Standalone Android 10 checks now cover in-app Stop/Start, Stop with notifications
disabled, permission recovery, keyboard-visible text and cursor editing, node
address sharing, and small direct messages in both directions. Identity, pairing,
contact, and mailbox retention have bounded app-process restart evidence. Valid
identity import also passes through the actual JNI runtime in isolated test
storage; the interactive picker check does not replace the app's saved identity.
See [validation results](validation.md#later-standalone-android-acceptance) for
the exact scope and checkpoint boundaries.

The September 9 clean combined build includes ordered radio recovery, both
L2CAP buffer corrections, GATT client-queue cleanup, and Describe's updated
readiness deadline. Twenty-two physical radio off/on cycles completed with a
successful first node check and no repeated missing-MTU stall. Unlike the earlier
diagnostic trials, these checks had no added two-second harness delay. Some
result captures required manual confirmation after harness guards paused; this
is not a claim of twenty-two automatic script passes.

On that same clean build, Stop/Home/return stayed stopped, explicit Start's
first check succeeded, the deleted contact remained absent after a cold app
restart, both actual-JNI tests passed, and a fresh two-way message exchange and
node-address share passed. Controlled offline tests also passed: retry/cancel
while stopped did not start the service, failed/cancelled messages did not send
on reconnection, and an explicit retry delivered the original message without
creating another mailbox record. These are bounded application-observation
checks, not exactly-once wire-delivery guarantees. The final copy-only rebuild
also passed its gates and an install/cold-launch smoke check, preserving message
states, contacts, and pairing; its first node check succeeded. The Android 10
implementation of the current iOS app feature set is ready for development
handoff with the limits below.
See the [clean combined
checkpoint](validation.md#clean-combined-checkpoint--september-9) for exact builds,
evidence, and the preceding failed comparisons. This is bounded evidence on one
Galaxy S9+/Android 10 and one board, not universal Android qualification. Newer
Android permission/service behavior and deep Doze remain unqualified.

### Connection readiness and bounded requests

An enabled Bluetooth permission or a stored route does not establish that its
interface is ready. Before a RemoteControl connection, the app checks the
route's actual interface for online, transmit-capable status. Describe waits
within the time remaining in its original 20-second admission deadline;
address sharing keeps its separate five-second readiness wait. A usable route
connects directly; otherwise an available transmitting interface permits one
path request for the authorized target. After discovery, the route's interface is
checked again. There is no automatic replay of the RemoteControl operation or
of path discovery when an interface later appears.

This readiness check uses the status published by the current Bluetooth and
TCP transports. It is not a general readiness guarantee for arbitrary custom
interfaces that do not publish that status, nor a guarantee that a ready peer
will answer.

Describe's native command has one 20-second deadline starting at admission,
including time spent queued, waiting for a transport, connecting, and reading
the response. Readiness does not start a second 20-second allowance or impose
an unconditional delay. The clean checkpoint above includes this follow-up.
The actor drops its Describe future if that deadline expires,
the native caller leaves, or Stop is requested, and releases its active-operation
and admission state. An expired queued command does not begin network work.
Generated async bindings accept an `AbortSignal`, and the SDK forwards it,
including Effect interruption. Aborting that caller releases Describe's
app-owned future without stopping the native node. Merely discarding a Promise
does not abort it. Caller cancellation also does not undo an accepted durable
write or independently spawned native work.

Once an authorized target connection has been returned, an app-owned guard
queues link closure on normal completion or cancellation. Before that point,
an upstream link may still be establishing or identifying: dropping its waiter
does not prove that the engine immediately cancelled the issued work. Engine
settlement/expiry and full node shutdown remain separate boundaries. Address
sharing retains its existing admitted-operation and unknown-outcome handling;
it is not given Describe's caller-cancellation semantics.

## Acceptance sequence

1. Build/link/package checks; shared SDK/UI and Rust tests; Android callback tests.
2. Real-device startup, generated/imported identity reopen in isolated native
   tests, picker preview/cancel/rejection without replacing an existing identity,
   contact pin/unpin/delete, and controlled TCP RemoteControl/LXMF communication.
3. Bluetooth discovery, invitation/code approval, authenticated remote request,
   and message delivery with the intended board.
4. UI recreation, screen locking, denied/regranted permissions, Bluetooth toggles,
   peer loss/recovery, both in-app and notification Stop followed by in-app
   Start, and process restart. Verify that ordinary navigation/resume after
   Stop does not start the service, and that Start retains pairing and mailbox
   data. Check in-app Stop with notifications disabled, and notification
   permission/channel recovery separately. Submit the first node check while
   reconnecting, not only after the interface is already ready. Record failures
   as well as successful retries. Repeat with bundled JavaScript and Metro stopped.

Passing compilation or emulator tests does not qualify physical Bluetooth or
background delivery. Current results are recorded in `validation.md`.

### Controlled physical messaging peer

`services/lxmf/interop/python/run_physical_tcp_lxmf.py` runs the pinned Python
peer without substituting a desktop Rust app for the phone. Its Python
environment and dependency pins are defined in `ci/verify_lxmf.sh`.
Use a concrete `--listen-ip`, an available `--port`, a bounded
`--timeout-seconds`, and `--expected-destination` set to the test phone's exact
32-hex-character **lxmf.delivery destination**, not its primary or controller
identity hash. The peer ignores other destinations when selecting a recipient
and when checking incoming messages. Omitting this option retains the
first-observed-peer behavior used by isolated host tests.

With an explicit expected destination, the peer also requests that destination's
path while it is unknown, at most once every ten seconds within the exchange
deadline. It accepts a validated path response only for that expected identity
association. This can discover a previously announced phone through a transport
board without touching the phone UI. Host auto-selection does not request paths
or opt into path responses. Record this extra discovery traffic when interpreting
background checks; discovery is not a delivery proof or a passive recovery test.

The standalone Android build has Bluetooth enabled but no runtime TCP editor.
A connected transport board can forward between Bluetooth and its configured
TCP interface to reach the peer. Confirm that route and its endpoints before
starting; do not change normal board or daemon configuration implicitly.

In Inbox, select **Share messaging address** and wait for the peer's
`PINNED_PYTHON_LXMF_RUST_OBSERVED` marker for the expected destination. The
incoming message must show title `Python`, body `python-to-rust`, and
**Verified source**. Reply to the peer destination printed at startup with
title `Rust` and body `rust-to-python`. Require **Delivered** in the app and
`PINNED_PYTHON_LXMF_OK inbound=verified outbound=proof links=two` from the peer.
The fixture exits and removes its temporary peer state after completion or
timeout; app messages are retained. This checks small direct Link messages,
not Resources, propagation, or background delivery.

For a locked-delivery check, add `--outbound-delay-seconds 30` (finite, at least
one second, and less than the exchange timeout). The delay starts when the
expected phone's announcement is observed. Record that the secure keyguard is
showing and the screen is off before the delay elapses; record those states
again after the peer emits `PINNED_PYTHON_LXMF_OUTBOUND_PROOF`. The one-shot
`OUTBOUND_SUBMITTED` and `OUTBOUND_PROOF` markers include UTC timestamps and the
same packed message hash. These are fixture observation times, not exact radio
transmission or storage-commit times. After unlocking, verify that exact message
hash in the inbox and its source-verification state before claiming app receipt.
Record charging/Doze state separately; a short USB-powered run does not establish
battery-powered idle reliability. The host gate always uses its default delay,
regardless of an exported physical-test delay.

For an unplugged trial, correlate the phone's recorded battery history after
reconnection with the peer's proof time. Retain the history's wall-clock anchor,
any clock adjustments, and charging, screen, and device-idle transitions; unrelated
application events are not needed. Distinguish `device_idle=light` from deep idle
and from the separate `screen_doze` display state. The history anchor has only
whole-second precision, and screen-off history does not establish secure keyguard
state. Still match the exact message in the inbox before recording app receipt.
