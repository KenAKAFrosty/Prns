# AutoWifi service-discovery evidence

Implementation commit: `c08e70e302d9a3d6d944be9f0febd4dd59737cc2`

Evidence date: 2026-08-17 UTC

This is focused current-host and physical-device evidence for the AutoWifi
DNS-SD discovery layer. The implementation commit was identical to
`origin/trunk`, and the tracked worktree was clean before qualification and
device artifacts were built.

## Contract exercised

- DNS-SD service type: `_reticulum._tcp.local.`
- TCP rendezvous port: `42699`
- TXT record: `v=1`
- Native host discovery capacity: 128 services
- Apple and Android discovery capacity: 255 services
- Candidate capacity: 8 addresses per service

The service capacities enter the shared channel as `NonZeroU8`; the shared
snapshot and advertisement types enforce those service and candidate bounds.

## Toolchain and artifacts

- macOS 26.4, build 25E246
- Xcode 26.6, build 17F113
- `rustc 1.96.0 (ac68faa20)`, `aarch64-apple-darwin`, LLVM 22.1.2
- `prnsd/target/release/prnsd`
  - SHA-256: `8aae00f9f20e062ccfca6c26f496177b8458ead0494b3a94a67e7e846c6e8fcf`
- Android lab APK:
  `personal-hopspot/mobile/android/app/build/outputs/apk/wifiDirectLab/app-wifiDirectLab.apk`
  - package: `org.personal.hopspot.wifidirectlab`
  - SHA-256: `838a3f9f1014869120da256ab0d0f9560f59248a693dfe57689b1a8df0f4f6c9`
- iOS lab application:
  `/private/tmp/prns-auto-wifi-ios-c08e70e3/Build/Products/Debug-iphoneos/PersonalHopspot.app`
  - bundle identifier: `org.personal.hopspot.ioslab`
  - binary SHA-256: `cf34b547b5590327f9623d4708633bde8500ca1f6d5de91de4d212c4d241e80e`
  - the application signature and embedded profile both used the isolated lab
    identifier; the profile explicitly included iPad and was valid through
    2026-08-24

Production mobile package identifiers were not replaced or modified.

## Current-host validation

Command:

```text
python3 validation/run.py run --tier pr --platform current
```

Result: all 24 selected suites passed with exit status 0. The run covered the
Apple platform suite, integration capstones, iOS simulator, JavaScript AutoWifi
reconnect behavior, root Clippy, root tests, validation self-tests, and the
Wasm AutoWifi, events, casework, and WebSocket suites. The validation
self-test's deliberate one-second timeout fixture behaved as expected and its
suite passed. No persistent browser-rendezvous timeout occurred in this run.

## Pixel 8

Device coverage:

- Pixel 8 (`shiba`)
- Android 16 / API 36.1
- build `CP1A.260505.005`
- arm64-v8a

The isolated lab service reported that discovery was bounded to 255 services
and 8 candidates per service, acquired its multicast lock, browsed
`_reticulum._tcp`, advertised TXT `v=1`, and listened on `0.0.0.0:42699`.

With the Pixel at `192.168.4.29` and the Mac at `192.168.4.35`, simultaneous
socket inspection proved two separate established links:

- Mac ephemeral port to Pixel port 42699, proving native host discovery and
  dialing of the Android advertisement.
- Pixel ephemeral port to Mac port 42699, proving Android discovery and dialing
  of the native host advertisement.

The Mac diagnostic log independently recorded discovery and connection of
`192.168.4.29:42699` at 2026-08-17T03:59:12Z.

A forced stop removed the Android listener and both links. Because a hard stop
cannot send a DNS-SD goodbye, the host briefly retried the cached endpoint as
expected. A cold relaunch restored both directed links. For the graceful path,
the Activity was first removed so it no longer bound the service, then the
lab-only stop action was delivered from the package sandbox. Android logged
multicast-lock release, the service and port 42699 listener disappeared, both
Mac sockets disappeared, and host retries ceased after two transition-time
failures when the removal snapshot arrived.

The lab package was uninstalled after the run. The previously installed
`org.personal.hopspot` package remained installed.

Raw local artifacts:

- `validation-artifacts/auto-wifi-discovery/c08e70e3/android-initial-logcat.log`
- `validation-artifacts/auto-wifi-discovery/c08e70e3/android-rejoin-teardown-logcat.log`
- `validation-artifacts/auto-wifi-discovery/c08e70e3/android-mac-prnsd.log`

## iPad Pro

Device coverage:

- iPad Pro 12.9-inch (4th generation), product `iPad8,11`
- iPadOS 18.7.8, build 22H352
- physical device identifier recorded only by suffix `802E`

The signed isolated lab application advertised `iPad (3)._reticulum._tcp`.
Independent `dns-sd` resolution returned port 42699 and TXT `v=1`. The native
Mac central advertised `prns-08409a88._reticulum._tcp`, also on port 42699 with
TXT `v=1`.

With the iPad at `192.168.4.32` and the Mac at `192.168.4.35`, simultaneous
socket inspection showed:

```text
192.168.4.35:42699 -> 192.168.4.32:51620
192.168.4.35:63668 -> 192.168.4.32:42699
```

The first socket proves the iPad discovered the Mac advertisement and dialed
the Mac central listener. The second proves the Mac discovered the iPad
advertisement and dialed the iPad listener. The iPad's aggregate AutoWifi
status (`0cde82ef`) became `Connected` and reported nonzero traffic, first
`rx=10961 tx=8867` and later `rx=14742 tx=15987`.

Process termination emitted removal events for the iPad service on interfaces
10, 17, and 16. iPadOS then restored the application for its registered BLE
restoration lifecycle, and the service was re-added on all three interfaces.
Both directed AutoWifi sockets returned with new ephemeral ports:

```text
192.168.4.35:63944 -> 192.168.4.32:42699
192.168.4.35:42699 -> 192.168.4.32:51737
```

This exercises native-backend drop, advertisement removal, automatic process
recreation, and discovery rejoin. The host JSONL log also records the matching
TCP server-peer and client transitions from connected to disconnected and back
to connected.

For a stable final teardown, only `org.personal.hopspot.ioslab` was uninstalled.
All three iPad service records were removed at 2026-08-17T14:32:00Z, no record
was re-added during the following five-second observation window, and both
device links disappeared. Socket inspection then showed only the Mac
`*:42699` central listener. The previously installed `com.personal.hopspot` application
remained installed.