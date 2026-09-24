# MetalbeardMobile installation and iOS 27 startup blocker

Resolved later the same day by the [scene-lifecycle migration](2026-09-22-ios-scenes.md).
This checkpoint preserves the earlier failed build and its evidence. The build
output directory below was reused for the successful replacement; the hashes
here identify the original binary, not the directory's current contents.

## Completed

`25f31b4fe` changes all 14 Back-to links to a decorative left arrow before the
label. Forward navigation stays unchanged. Router tests cover ordering,
accessibility, touch height and explicit destinations. All 320 app tests pass,
as do formatting, lint, source/test/tools typechecks, route/config checks and the
Swift platform tests. No changes were pushed.

A development-identifier (`rs.reticulum.prns.dev`) Release build was generated,
signed with the existing team/profile and installed over the existing app on
USB-connected MetalbeardMobile, without uninstalling or clearing data. The native
Rust framework was also built in Release mode. The JavaScript bundle is embedded;
this build does not require Metro. Xcode compilation and deep signature checks
passed, as did bundle identifier, iOS 18 minimum, ASK and central-only metadata
checks. No firmware or board settings were changed.

Source: `25f31b4fe`. SHA256 values:

- App executable: `f7680f0be49d530220260de450207a56348871830c4983e380b2547a5857828f`
- JavaScript: `b319276faf4b0b484d58a113206fd7e0289f204fa5a88715f479c391fdbc9c00`
- Rust framework: `a9a19889ded65a542523edaac4f85728d2d0b6995435a0313590f0b0fb6522d3`

Build output is on Wavlink at
`/Volumes/wavlink/dev/prns-ios-metalbeard-20260922.FotaWv/Build/Products/Release-iphoneos/prnsdev.app`.
Logs, installation receipt, metadata verification and copied device crash reports
are under `scratch/prns-app/2026-09-22/ios-install/`.

## Startup is blocked; this is not usable-device acceptance

The phone is now on iOS 27 and the installed toolchain is Xcode 27. Launch through
both developer tools and iPhone Mirroring exits immediately. The device report
shows `SIGTRAP` in
`___UIApplicationEvaluateRuntimeIssueForNoSceneLifecycleAdoption_block_invoke`.
The developer-tool console misleadingly reports exit code zero; the copied crash
report and visible exit are the authoritative result. Device reports were copied,
not removed.

[Apple requires scene-based startup for apps built with the iOS 27 SDK](https://developer.apple.com/documentation/uikit/transitioning-to-the-uikit-scene-based-life-cycle).
[Expo's SDK 57 migration option](https://github.com/expo/fyi/blob/main/ios-scene-lifecycle.md#staying-on-sdk-57-with-xcode-27)
is supported by the installed Expo 57.0.23 runtime and
`expo-build-properties` 57.0.21. However, enabling scenes alone would break our
current Bluetooth restoration gate: scene-based launches supply nil AppDelegate
launch options, while the coordinator depends on `.bluetoothCentrals`.
[Apple's restoration guidance](https://developer.apple.com/documentation/corebluetooth/central-manager-state-restoration-options)
instead requires recreating the central with its stable identifier on launch.

Preliminary scene-plugin edits were withdrawn pending approval for the broader
native lifecycle change. The tracked tree retains only the completed arrow fix
and this report, not a partially enabled scene lifecycle. No successful iOS UI,
retained-pairing read, background restoration or enlarged-text check is claimed.

## Proposed follow-up

1. Enable Expo's supported single-scene startup and replace obsolete no-scene
   build/configuration assertions.
2. Keep native startup process-owned, before and independent of React/scene
   creation. Replace launch-reason detection with explicit startup eligibility,
   preserving identity readiness, ASK authorization, stable restoration identity,
   off-main preparation, generation coalescing and stop/reset cancellation.
3. Test nil launch options, missing/locked identity, authorization changes,
   concurrent starts, stop/reset races and protected-data retries. Diagnostics
   must distinguish an attempted restoration from an observed restored state.
4. Rebuild/install over retained data; verify cold launch and the back links,
   then separately qualify Bluetooth restoration and scene/background behavior.
