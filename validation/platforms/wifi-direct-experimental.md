# Wi-Fi Direct experimental transport

## Status

Wi-Fi Direct is experimental. It is not a production-supported Prns transport,
is not part of the Android production hardware gate, and does not run in a
default Android build.

The retained implementation is a laboratory surface for protocol work,
simulation, and future platform evaluation. A passing unit test or hwsim run
does not establish Android-to-Linux interoperability.

Android operators can opt in only at build time:

```bash
cd personal-hopspot/mobile/android
./gradlew :app:assembleWifiDirectLab
```

The lab APK uses the application ID
`org.personal.hopspot.wifidirectlab`, isolated storage, and a non-exported
service. It can coexist with an independently signed production or development
installation without replacing that installation or its identity.

Install, launch, and remove only the isolated lab package:

```bash
adb -s "${ANDROID_SERIAL}" install -r \
  app/build/outputs/apk/wifiDirectLab/app-wifiDirectLab.apk
adb -s "${ANDROID_SERIAL}" shell am start -W -n \
  org.personal.hopspot.wifidirectlab/org.personal.hopspot.MainActivity
adb -s "${ANDROID_SERIAL}" shell am force-stop \
  org.personal.hopspot.wifidirectlab
adb -s "${ANDROID_SERIAL}" uninstall org.personal.hopspot.wifidirectlab
```

Uninstalling the lab removes only its isolated identity and storage. It does
not alter an installed `org.personal.hopspot` app or that app's persistent
state.

`assembleProduction` rejects the legacy
`-PprnsExperimentalWifiDirect=true` opt-in. The standard Android validation
also asserts that the property is false.

## Established behavior

The reusable parts of the implementation have automated coverage:

- the service role is encoded as `Prns-native` or `Prns-supplicant`;
- DNS-SD PTR records are encoded and decoded by one shared implementation;
- peer addresses and group-owner intent cross the Android JNI boundary as one
  fixed-shape request;
- unknown or malformed requests are rejected;
- Android formation has a bounded deadline and reports failure to Rust;
- Android service shutdown clears discovery, pending negotiation, and group
  state;
- incoming supplicant negotiation is authorized rather than initiated a
  second time; and
- two isolated `mac80211_hwsim` nodes exercise the Linux discovery and group
  lifecycle.

Run the isolated Linux proof from the repository root:

```bash
bash validation/platforms/wifi-direct-hwsim-smoke.sh
```

Success ends with `WIFI_DIRECT_HWSIM_OK`. This proof requires `sudo`, loads
`mac80211_hwsim`, and must leave the real radio untouched.

## Hardware findings

### Android to Android

The 2026-07-23 two-device investigation used two modern Android phones. It
established:

1. Both phones discovered the typed Prns service and started group formation.
2. Android displayed a system Device connection approval dialog on the
   receiving phone.
3. Without approval, the bounded formation deadline expired and reported
   failure without hanging the service.
4. After approval, a fresh automatic retry still did not reach a created group
   or exchange a payload within one minute.
5. While group formation was active, Wi-Fi Aware attach attempts failed on
   both phones.
6. Removing the lab package and restarting the default app restored an
   automatic Wi-Fi Aware data path and the LAN connection on both phones.

[`WifiP2pManager`](https://developer.android.com/reference/android/net/wifi/p2p/WifiP2pManager)
can defer an incoming Wi-Fi Direct decision to the Wi-Fi service, which
displays a user dialog. Programmatic external approval requires
`MANAGE_WIFI_NETWORK_SELECTION`; the
[Android platform manifest](https://android.googlesource.com/platform/frameworks/base/+/master/core/res/AndroidManifest.xml)
reserves that permission for trusted platform or OEM apps and explicitly
excludes third-party applications. A normal independently distributed Prns app
cannot safely suppress this system approval boundary.

[Wi-Fi Aware](https://developer.android.com/develop/connectivity/wifi/wifi-aware)
does not use that per-peer Wi-Fi Direct approval flow. It remains a production
transport behind the standard Nearby devices runtime permission.

### Android to Linux

The 2026-07-23 Android-to-Linux investigation used a modern Android device and
Linux `wpa_supplicant` 2.10 through its system D-Bus API. It established:

1. Android and Linux discovered the typed Prns DNS-SD service.
2. Linux received the Android push-button and GO-negotiation requests.
3. D-Bus authorization allowed WPS and GO negotiation to complete.
4. Android requested group-owner intent `13` while Linux advertised preference
   `2`, yet Linux became Group Owner.
5. The Linux group received neither the conventional group-owner IPv4 address
   nor a usable IPv6 link-local address because the system path provided no
   address or DHCP orchestration.
6. The transient group object disappeared from D-Bus before dependable
   teardown, while its kernel netdev could remain.
7. Falling back to disconnecting the base P2P device interrupted the laptop's
   ordinary station connection. That fallback was removed.

No bidirectional Prns payload crossed the Android-to-Linux data plane. The
hardware result is therefore a failed interoperability proof, not partial
production evidence.

Android documents peer connection and invitation behavior in
[`WifiP2pManager`](https://developer.android.com/reference/android/net/wifi/p2p/WifiP2pManager).
The Linux operations used the
[`wpa_supplicant` D-Bus P2P API](https://w1.fi/wpa_supplicant/devel/dbus.html).
Each API exposes pieces of the workflow, but neither owns cross-platform role
selection, IP provisioning, DHCP, transient-interface cleanup, and coexistence
with the ordinary station connection as one contract.

## Laboratory boundary

Use dedicated test devices and record the station and interface state before
each run:

```bash
iw dev
nmcli device status
```

Stop the Android app before Linux cleanup. Remove only an interface whose exact
name was created by the current run:

```bash
adb -s "${ANDROID_SERIAL}" shell am force-stop org.personal.hopspot
sudo iw dev "${P2P_GROUP_INTERFACE}" del
```

Never substitute the station interface, the base P2P management device, a
wildcard, or a broad disconnect operation. A test fails if it interrupts an
unrelated network connection, leaves a group netdev behind, requires an
unbounded wait, or needs manual recovery not recorded with the evidence.

Do not commit device identifiers, MAC addresses, SSIDs, pairing material,
screenshots, captures, or logs from personal hardware.

## Re-entry criteria

Resume production evaluation only when a proposed design supplies evidence for
all of these properties:

- deterministic role handling or a role-independent data-plane design;
- unprivileged and explicit address and DHCP ownership on Linux;
- group teardown that cannot disconnect the station interface;
- discovery and payload exchange across at least two Android vendors and two
  Linux Wi-Fi chipsets;
- denial, sleep, wake, process death, restart, and repeated-formation recovery;
- no orphaned netdev, listener, supplicant object, or platform task;
- stable operation alongside the device's normal Wi-Fi connection; and
- a sustained bidirectional Prns traffic run.

Until then, production work should use Wi-Fi Auto/LAN, BLE, USB, and the
platform-supported Wi-Fi Aware path where available.
