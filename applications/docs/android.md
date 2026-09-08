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
ongoing notification and an explicit Stop action. Android may still terminate
the process. A sticky service restart reopens the stored identity and the last
successful start configuration; a user Stop clears that restart intent.

Identity, grants, contacts, and messages remain Rust-owned under the application's
private `noBackupFilesDir/prns/development` directory. These are disposable
development records, not a secure-storage or migration guarantee.

Bluetooth uses the public upstream Android backend and an attributed adaptation
of its Kotlin transport glue. Android permissions and discovery are independent
of Apple AccessorySetupKit and of RemoteControl invitation/grant pairing. The
node and TCP interface can start without granting Bluetooth permission.

## Build

Install the Android SDK/NDK, JDK 21, the repository Rust toolchain, and the Rust
`aarch64-linux-android` target. Set `ANDROID_HOME` to the SDK directory and install
the application JavaScript dependencies as described in the app README.

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

## Acceptance sequence

1. Build/link/package checks; shared SDK/UI and Rust tests; Android callback tests.
2. Real-device startup, identity creation/reopen, contacts, and controlled TCP
   RemoteControl/LXMF communication.
3. Bluetooth discovery, invitation/code approval, authenticated remote request,
   and message delivery with the intended board.
4. UI recreation, screen locking, denied/regranted permissions, Bluetooth toggles,
   peer loss/recovery, explicit Stop, and process restart. Record failures as well
   as successful retries. Repeat with bundled JavaScript and Metro stopped.

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
