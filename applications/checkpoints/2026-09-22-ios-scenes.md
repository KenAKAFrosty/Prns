# iOS scene lifecycle and MetalbeardMobile acceptance

Source: `fe028a77f` on `prns-app`, following the back-link fix `25f31b4fe`.
This resolves the [earlier iOS 27 launch failure](2026-09-22-ios-install.md).
Changes are committed locally; nothing was pushed during this work.

## Implementation

Expo SDK 57's supported scene opt-in uses `expo-build-properties` 57.0.21 and
`EXExpoAppSceneDelegate`. Generated and compiled metadata checks require one
scene configuration. The AppDelegate supplies the React Native factory; the
scene owns the window. Expo remains at 57.0.23, without a general dependency bump.

Native startup remains process-owned, independent of React and scene connection.
Each process launch requests restoration preparation with the stable Bluetooth
identifier, rather than relying on launch options unavailable with scenes.
Preparation requires an existing valid primary identity and accessory
authorization. Missing storage or identity cannot create onboarding files, a BLE
identity or a central manager. Off-main preparation, generation coalescing,
Stop/reset cancellation and bounded protected-data recovery remain in place.
Diagnostics distinguish a restoration attempt from an actual restored state.

The optional development TCP target is now compiled into Debug iOS configuration
so native startup and JavaScript cannot disagree. Changing it requires rebuilding
the client, not just restarting Metro. Release builds exclude the fixture.
Android's fixture behavior and the generated binding ABI are unchanged.

## Automated checks

- 321 app tests in 38 suites; 51 SDK tests in five suites.
- 189 native unit tests and four native integration tests.
- Swift dispatch, recovery, restoration diagnostics and release-symbol checks.
- App/SDK formatting, lint and source/test/tools typechecks; version/config checks.
- Generated API checks, iOS bridge guards and nine scene-metadata fixtures.
- Scoped Rust formatting, Release framework build, clean iOS prebuild and Pods.
- Release Xcode build, deep signature and compiled bundle metadata verification.

One dependency advisory remains: `expo install --check` exited nonzero because
newer SDK 57 patches were recommended (`expo` 57.0.24, `expo-router` 57.0.22,
`expo-constants` 57.0.19 and `@expo/metro-runtime` 57.0.16). That refresh was not
included in this migration. The installed Expo 57.0.23 supports the scene opt-in.
This is not a claim that the complete repository or publishing matrix was run.

## Installed binary

MetalbeardMobile: iOS 27, built with Xcode 27; bundle `rs.reticulum.prns.dev`,
iOS 18 minimum, team `MWDZ9M6W55`, existing development provisioning profile
`08e00f9b-93e1-4beb-9382-e8ac7956cfc9`. Both app and Rust framework are Release
builds. JavaScript is embedded; Metro is not needed. Central-only Bluetooth and
AccessorySetupKit metadata passed inspection. Installation retained existing data.

SHA256 values:

- App executable: `981b64045a3bc2af205324e689745d2e64e1fdd76c252ae6c9028f915a600517`
- JavaScript: `ce256c303338a7c47f2e4160cd43dc08c947d1559335ca8ddce34fc643d8eb32`
- Rust framework: `85619a56289069486d12a563b4e7cd00815dc5a76f40b5e7b2c941a9d6272076`

The replacement build occupies
`/Volumes/wavlink/dev/prns-ios-metalbeard-20260922.FotaWv/Build/Products/Release-iphoneos/prnsdev.app`.
Local logs/receipts are in `scratch/prns-app/2026-09-22/ios-install/`; the
`scenes-*` receipts and `scenes-device.log` distinguish this build from the
earlier failed installation. Device screenshots were inspected in the task,
not saved as checkpoint artifacts.

## Physical observations and limits

At 20:52:17 EDT, PID 15061 logged launch, accessory authorization, successful
preparation and native startup. Nodes showed the saved pairing and a Running
local instance. Two saved contacts and the existing 13-message conversation
were visible. Back to Nodes and Back to Contacts showed leading left arrows and
returned to the expected routes.

Leaving for the Home screen and reopening retained PID 15061 without a second
native start. A deliberate developer-tool cold relaunch at 20:55:35 EDT created
PID 15066, again logged preparation/start success, and retained the pairing and
13 messages. This was a foreground cold restart, not an OS Bluetooth wake.

No identity reset, uninstall, contact save, message send, firmware flash or board
setting change was performed. The Bluetooth interface was disconnected with no
traffic or live routes. Consequently these checks do not establish remote
read/write, fresh pairing, restored Bluetooth transport, background delivery,
natural suspension or long locked-idle behavior for this binary.

An unsaved contact name accepted typing through Mirroring, but no software
keyboard was displayed; this is not software-keyboard acceptance. Enlarged text
was not checked on iOS. No new Android binary was installed in this continuation.
Next device qualification should cover a live board connection, remote reads,
bounded background Bluetooth recovery and the remaining iOS UI checks.
