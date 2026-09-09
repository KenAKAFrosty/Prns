import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const swift = readFileSync(resolve(packageRoot, "ios/PrnsAppModule.swift"), "utf8");
const coordinator = readFileSync(
  resolve(packageRoot, "ios/PrnsAppLifecycleCoordinator.swift"),
  "utf8",
);
const protectedDataRecovery = readFileSync(
  resolve(packageRoot, "ios/PrnsProtectedDataRecovery.swift"),
  "utf8",
);
const restorationDispatch = readFileSync(
  resolve(packageRoot, "ios/PrnsRestorationDispatch.swift"),
  "utf8",
);
const accessoryCoordinator = readFileSync(
  resolve(packageRoot, "ios/PrnsAccessorySetupCoordinator.swift"),
  "utf8",
);
const subscriber = readFileSync(
  resolve(packageRoot, "ios/PrnsAppDelegateSubscriber.swift"),
  "utf8",
);
const restorationProbe = readFileSync(
  resolve(packageRoot, "ios/PrnsAppRestorationProbe.swift"),
  "utf8",
);
const iosDiagnostics = readFileSync(resolve(packageRoot, "ios/PrnsIosDiagnostics.swift"), "utf8");
const moduleConfig = JSON.parse(
  readFileSync(resolve(packageRoot, "expo-module.config.json"), "utf8"),
);
const podspec = readFileSync(resolve(packageRoot, "ios/PrnsApp.podspec"), "utf8");
const developmentClient = readFileSync(
  resolve(packageRoot, "ios/build-development-client.sh"),
  "utf8",
);
const iosDeploymentPlugin = readFileSync(
  resolve(packageRoot, "../../prns/app/tools/with-ios-18.ts"),
  "utf8",
);
const bindingsBuild = readFileSync(
  resolve(packageRoot, "../../tools/generated-bindings/generate.py"),
  "utf8",
);
const bindingsPod = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/bindings/NativeBindings.podspec"),
  "utf8",
);
const nativeCargo = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/Cargo.toml"),
  "utf8",
);
const nativeBindings = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/src/bindings/mod.rs"),
  "utf8",
);
const nativeLifecycle = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/src/lifecycle.rs"),
  "utf8",
);
const nativeRestorationProbe = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/src/ios_restoration_probe.rs"),
  "utf8",
);
const applicationsPackage = JSON.parse(
  readFileSync(resolve(packageRoot, "../../package.json"), "utf8"),
);
const compatibility = JSON.parse(
  readFileSync(resolve(packageRoot, "../../release/compatibility.json"), "utf8"),
);
const appConfig = readFileSync(resolve(packageRoot, "../../prns/app/app.config.ts"), "utf8");
const configCheck = readFileSync(
  resolve(packageRoot, "../../prns/app/tools/config-check.ts"),
  "utf8",
);

function assertOne(source, pattern, message) {
  const flags = pattern.flags.includes("g") ? pattern.flags : `${pattern.flags}g`;
  assert.equal([...source.matchAll(new RegExp(pattern.source, flags))].length, 1, message);
}

function captureOne(source, pattern, owner) {
  const matches = [...source.matchAll(pattern)];
  assert.equal(matches.length, 1, `${owner} must declare exactly one Bluetooth service UUID`);
  return matches[0][1];
}

const bluetoothServiceUuid = compatibility.prns?.bluetoothAuto?.serviceUuid;
assert.equal(
  typeof bluetoothServiceUuid,
  "string",
  "compatibility must record the validated Bluetooth Auto service UUID",
);
assert.match(
  bluetoothServiceUuid,
  /^[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}$/,
  "compatibility Bluetooth Auto service UUID must be canonical uppercase",
);
for (const [owner, source, pattern] of [
  ["app.config.ts", appConfig, /\bconst prnsBluetoothServiceUuid = "([^"]+)";/g],
  ["config-check.ts", configCheck, /\bconst prnsBluetoothServiceUuid = "([^"]+)";/g],
  [
    "PrnsAccessorySetupCoordinator.swift",
    accessoryCoordinator,
    /\bCBUUID\(\s*string: "([^"]+)"\s*\)/g,
  ],
  ["build-development-client.sh", developmentClient, /^PRNS_BLUETOOTH_SERVICE="([^"]+)"$/gm],
]) {
  assert.equal(
    captureOne(source, pattern, owner),
    bluetoothServiceUuid,
    `${owner} must match the recorded canonical Bluetooth Auto service UUID`,
  );
}

assert.match(
  swift,
  /DispatchQueue\(\s*label: "rs\.reticulum\.prns\.app\.native",\s*attributes: \.concurrent\s*\)/,
  "native calls must run on a concurrent queue so Rust owns command admission",
);
assert.equal(
  swift.match(/\.runOnQueue\(Self\.nativeQueue\)/g)?.length,
  7,
  "the seven storage/identity/lifecycle transports must use the native operation queue",
);
assert.doesNotMatch(
  swift,
  /\bOnDestroy\b/,
  "Expo module teardown must not stop process-owned Rust",
);
assert.match(swift, /nativeQueue\.async\(flags: \.barrier\)/);
assert.match(
  coordinator,
  /PrnsAccessorySetupCoordinator\.shared\.activate\([\s\S]*?restorationLaunchRequested: centralRestoration[\s\S]*?prepareAndStartNativeRuntime/,
  "every launch must activate ASK before restoration creates a CoreBluetooth manager",
);
assert.match(
  `${accessoryCoordinator}\n${coordinator}`,
  /claimIfAuthorized\(nativeStartAuthorized\)[\s\S]*?restorationReady\(\)[\s\S]*?requireAuthorized\(\)[\s\S]*?prepareBluetoothCentralRestoration\(\)[\s\S]*?case \.prepared, \.alreadyPrepared:[\s\S]*?startNativeRuntime/,
  "restoration must wait for ASK activation plus an authorized Bluetooth accessory",
);
const iosPreparedBluetooth =
  nativeLifecycle.match(
    /#\[cfg\(all\(feature = "apple", target_os = "ios"\)\)\]\s+let prepared_bluetooth =[\s\S]*?#\[cfg\(all\(feature = "apple", target_os = "macos"\)\)\]\s+let prepared_bluetooth =/,
  )?.[0] ?? "";
assert.notEqual(
  iosPreparedBluetooth,
  "",
  "the static check must locate the complete iOS Bluetooth preparation branch",
);
assert.match(
  iosPreparedBluetooth,
  /AutoBle::prepare_central_only_without_restoration[\s\S]*?AutoBle::unavailable_central_only_without_restoration/,
  "even an unprepared iOS start must remain central-only",
);
assert.doesNotMatch(
  iosPreparedBluetooth,
  /AutoBle::(?:prepare|unavailable)_without_restoration/,
  "the iOS path must never initialize the dual-role Bluetooth backend",
);
assert.match(
  coordinator,
  /restorationLaunchIdentifiers[\s\S]*?as\? \[String\][\s\S]*?as\? NSArray[\s\S]*?compactMap/,
  "restoration launch identifiers must accept native Swift arrays and bridged NSArray values",
);
assert.match(
  coordinator,
  /private func waitForProtectedData\(application: UIApplication\)[\s\S]*?addObserver\([\s\S]*?guard !application\.isProtectedDataAvailable else \{[\s\S]*?stopWaitingForProtectedData\(application: application\)[\s\S]*?prepareAndStartNativeRuntime\(application: application\)/,
  "protected-data waiting must register before rechecking and retry immediately after a lost wake",
);
assert.match(
  coordinator,
  /private func prepareAndStartNativeRuntime\(application: UIApplication\) \{\s+do \{\s+try PrnsAccessorySetupCoordinator\.shared\.requireAuthorized\(\)[\s\S]*?protectedDataRecovery\.beginAttempt\([\s\S]*?prepareBluetoothCentralRestoration\(\)/,
  "every initial and resumed attempt must revalidate ASK immediately before manager preparation",
);
assert.doesNotMatch(
  coordinator,
  /private func prepareAndStartNativeRuntime\(application: UIApplication\) \{\s+guard application\.isProtectedDataAvailable/,
  "ordinary after-first-unlock relocks must still attempt restoration against accessible storage",
);
assert.match(
  protectedDataRecovery,
  /let eligible = Self\.isRecoveryEligible\(failure\)[\s\S]*?let observedUnavailable = unavailableObserved[\s\S]*?guard !consumed, eligible, observedUnavailable[\s\S]*?consumed = true/,
  "protected-data recovery must require observed unavailability and consume one bounded retry",
);
assert.match(
  protectedDataRecovery,
  /stage == "storage" \|\| stage == "identity" \|\| stage == "persistenceRestore"/,
  "only storage-adjacent native failures may trigger protected-data recovery",
);
assert.match(
  coordinator,
  /recoverAfterProtectedDataFailure\([\s\S]*?requireAuthorized\(\)[\s\S]*?restorationStartAuthorizationDidClose\(\)[\s\S]*?protectedDataRecovery\.action\(for: failure\)[\s\S]*?waitForProtectedData/,
  "a recovery must revalidate ASK and enter the race-safe protected-data wait",
);
assert.match(
  `${accessoryCoordinator}\n${restorationDispatch}`,
  /restorationStartAuthorizationDidClose\(\)[\s\S]*?rearmAfterAuthorizationLoss\(\)[\s\S]*?claimIfAuthorized[\s\S]*?guard launchRequested, authorized, !claimed/,
  "authorization loss must re-arm exactly one later restoration dispatch",
);
assert.match(swift, /\.appendingPathComponent\("prns", isDirectory: true\)/);
assert.match(swift, /\.appendingPathComponent\("development", isDirectory: true\)/);
assert.match(
  swift,
  /private static func restorationStorageURL\(\)[\s\S]*?migrateExistingContents: false/,
  "early restoration must avoid recursively migrating the existing storage tree",
);
assert.match(
  swift,
  /static func prepareBluetoothCentralRestoration\(\)[\s\S]*?nativePrepareAppleBluetoothCentralRestoration/,
  "the restoration hook must call the central-only preparation ABI",
);
assert.match(swift, /FileProtectionType\.completeUntilFirstUserAuthentication/);
assert.match(swift, /isExcludedFromBackup = true/);
assert.match(swift, /isSymbolicLink == true[\s\S]*skipDescendants\(\)/);
assert.deepEqual(moduleConfig.apple?.appDelegateSubscribers, ["PrnsAppDelegateSubscriber"]);
assert.match(subscriber, /willFinishLaunchingWithOptions/);
assert.match(coordinator, /\.bluetoothCentrals/);
assert.doesNotMatch(coordinator, /\.bluetoothPeripherals/);
assert.match(coordinator, /\.contains\(identifier\)/);
assert.doesNotMatch(`${swift}\n${coordinator}\n${accessoryCoordinator}`, /CBPeripheralManager/);
assert.equal(
  accessoryCoordinator.match(/ASAccessorySession\(\)/g)?.length,
  1,
  "the app must own exactly one ASK session",
);
assert.match(accessoryCoordinator, /session\.activate\(on: \.main\)/);
assert.match(
  accessoryCoordinator,
  /session\.accessories\.filter[\s\S]*?\.state == \.authorized && \$0\.bluetoothIdentifier != nil/,
  "native startup must derive authorization from the activated ASK session",
);
assert.match(accessoryCoordinator, /UIApplication\.shared\.applicationState == \.active/);
assert.match(accessoryCoordinator, /session\.showPicker\(for: \[Self\.pickerDisplayItem\]\)/);
assert.match(
  accessoryCoordinator,
  /guard nativeStartPhase != \.starting, nativeStartPhase != \.stopping else \{\s+completion\(Self\.pickerOutcome\(type: "notReady"\)\)/,
  "the native chooser must reject presentation while start or stop ownership is changing",
);
assert.match(
  accessoryCoordinator,
  /case \.accessoryAdded, \.accessoryChanged, \.accessoryRemoved:\s+reconcileAuthorizedAccessories\(\)[\s\S]*?case \.pickerDidDismiss:\s+pickerDismissalPending = false\s+pickerPhase = \.idle\s+reconcileAuthorizedAccessories\(\)/,
  "an added accessory must remain gated until ASK reports picker dismissal",
);
assert.match(
  accessoryCoordinator,
  /if pickerDismissalPending && nextAuthorizedAccessoryCount > authorizedAccessoryCount \{\s+return\s+\}/,
  "authorization increases must wait for picker dismissal while removals still reconcile",
);
assert.match(accessoryCoordinator, /37145B00-442D-4A94-917F-8F42C5DA28E3/);
assert.match(
  accessoryCoordinator,
  /nativeStartCompletions\.count < 8/,
  "duplicate native starts must be coalesced with a bounded waiter set",
);
assert.match(
  `${swift}\n${accessoryCoordinator}`,
  /requestNativeStart[\s\S]*?startGeneration[\s\S]*?requireAuthorized\([\s\S]*?startGeneration:[\s\S]*?startWithCentralRestoration/,
  "the app-owned gate must revalidate authorization and start ownership before entering Rust",
);
assert.match(
  swift,
  /AsyncFunction\("stop"\)[\s\S]*?beginNativeStop\(\)[\s\S]*?nativeStop\(\)[\s\S]*?finishNativeStop\(outcome\)/,
  "an explicit stop must invalidate the cached native start generation",
);
assert.match(
  `${swift}\n${accessoryCoordinator}`,
  /beginNativeStop\(\)[\s\S]*?nativeStopWillBegin\(application: \.shared\)[\s\S]*?nativeStopWillBegin\(\)[\s\S]*?restorationDispatch\.cancel\(\)/,
  "explicit stop must cancel both pending lifecycle recovery and restoration dispatch",
);
assert.match(
  coordinator,
  /case \.failure\(let error\):[\s\S]*?error is PrnsNativeStartInterruption \? \.cancelled : \.bridge[\s\S]*?if case \.cancelled = failure[\s\S]*?finishAttempt\(\)[\s\S]*?return/,
  "stop supersession must terminate before ASK re-arm or protected-data retry",
);
assert.match(
  swift,
  /AsyncFunction\("reset"\)[\s\S]*?beginNativeStop\(\)[\s\S]*?nativeReset[\s\S]*?finishNativeStop\(outcome\)/,
  "reset must invalidate the cached generation before a later onboarding start",
);
assert.match(
  accessoryCoordinator,
  /case \.alreadyStopped, \.stopped:[\s\S]*?nativeStartPhase = \.notRequested[\s\S]*?default:[\s\S]*?nativeStartPhase = \.stopping/,
  "only a definitive stopped outcome may reopen native start admission",
);
assert.match(accessoryCoordinator, /statusRevision &\+= 1[\s\S]*?statusJSON\(\)/);
assert.match(swift, /DevelopmentNodeStartInput\(developmentTcpTarget: nil\)/);
assert.doesNotMatch(
  `${coordinator}\n${subscriber}`,
  /applicationDidEnterBackground|applicationWillResignActive|prns_app_stop/,
  "background lifecycle hooks must not stop the native node",
);
assert.match(restorationProbe, /^#if DEBUG\nimport Foundation/);
assert.match(restorationProbe, /\n#endif\s*$/);
assert.match(
  restorationProbe,
  /@_cdecl\("prns_app_ios_restoration_probe_emit"\)/,
  "the private Rust callback must terminate in the Debug-only tagged sink",
);
assert.match(
  restorationProbe,
  /guard let event = PrnsIosDiagnostics\.RestorationEvent\(rawValue: code\)[\s\S]*?PrnsIosDiagnostics\.restoration\(sequence: sequence, event: event\)/,
);
assert.doesNotMatch(
  `${restorationProbe}\n${coordinator}\n${accessoryCoordinator}\n${iosDiagnostics}`,
  /\bNSLog\s*\(/,
  "diagnostics must not rely on NSLog being forwarded into the USB unified-log stream",
);
assertOne(iosDiagnostics, /\bLogger\(/, "one explicit unified-log sink owns these diagnostics");
assert.match(
  iosDiagnostics,
  /logger\.notice\("\\\(channel\.rawValue, privacy: \.public\) \\\(message, privacy: \.public\)"\)/,
  "message text must expose the payload-free tag and fields explicitly, not just category metadata",
);
assert.match(
  iosDiagnostics,
  /private static func emit\(_ channel: Channel, _ message: String\)/,
  "only the typed diagnostic entry points may reach the public unified-log sink",
);
assert.match(
  iosDiagnostics,
  /#if DEBUG\s+(?:\/\/[^\n]*\n\s*)*fputs\("\\\(channel\.rawValue\) \\\(message\)\\n", stderr\)\s+#endif/,
  "Debug builds must retain a direct stderr mirror for devicectl console capture",
);
assert.match(iosDiagnostics, /enum LifecycleEvent \{[\s\S]*?case nativeOutcome\(/);
assert.match(
  iosDiagnostics,
  /static func accessorySetup\(\s*phase: AccessorySetupPhase,[\s\S]*?restoration: Bool/,
);
assert.match(
  iosDiagnostics,
  /static func restoration\(sequence: UInt64, event: RestorationEvent\)/,
);
assert.doesNotMatch(
  `${restorationProbe}\n${coordinator}\n${accessoryCoordinator}`,
  /PrnsIosDiagnostics\.emit/,
  "callers must use typed diagnostic APIs rather than publish arbitrary strings",
);
assert.match(coordinator, /PrnsIosDiagnostics\.lifecycle\(event\)/);
assert.match(accessoryCoordinator, /PrnsIosDiagnostics\.accessorySetup\(/);
assert.doesNotMatch(
  `${restorationProbe}\n${nativeRestorationProbe}`,
  /peripheral_service_restored|bluetooth_auto::macos::peripheral/,
  "central-only restoration diagnostics must not claim peripheral-role restoration",
);
assert.match(
  restorationProbe,
  /codeLength > 0, codeLength <= 64[\s\S]*?RestorationEvent\(rawValue: code\)/,
  "the restoration sink must accept only bounded allowlisted event codes",
);
assert.match(
  nativeCargo,
  /ios-restoration-probe = \["apple", "dep:log", "prns-interfaces-tokio\/log"\]/,
  "the private restoration logger must remain an explicit app feature",
);
assert.match(
  bindingsBuild,
  /args.command == 'ios' and not args.release[\s\S]*?ios-restoration-probe/,
  "only Debug framework builds enable native restoration diagnostics",
);
assert.equal(nativeLifecycle.match(/crate::ios_restoration_probe::install\(\);/g)?.length, 2);
assert.match(
  subscriber,
  /PrnsAppRestorationProbe\.install\(\)[\s\S]*?PrnsAppLifecycleCoordinator\.shared\.launch/,
);
assert.match(nativeRestorationProbe, /prns_app_ios_install_restoration_probe[\s\S]*?EMITTER\.set/);
assert.doesNotMatch(nativeRestorationProbe, /fn prns_app_ios_restoration_probe_emit/);
assert.match(
  swift,
  /nativeStartWithAppleBluetoothCentralRestoration\([\s\S]*?storageRoot:[\s\S]*?input:[\s\S]*?centralIdentifier:/,
);
assert.match(
  swift,
  /nativePrepareAppleBluetoothCentralRestoration\([\s\S]*?storageRoot:[\s\S]*?centralIdentifier:/,
);
assert.match(
  swift,
  /FfiConverterTypeDevelopmentNodeStartInput\.read[\s\S]*?buffer.offset == buffer.data.endIndex/,
);
assert.match(swift, /FfiConverterTypeDevelopmentNodeStartOutcome\.write/);
assert.match(swift, /FfiConverterTypeDevelopmentNodeStopOutcome\.write/);
assert.doesNotMatch(
  swift,
  /prns_app_(?:start|stop|snapshot|bytes_free)|invokePathJSON|inputJSON|JSONSerialization/,
);
assert.match(nativeBindings, /#\[uniffi::export\][\s\S]*?pub async fn read_snapshot/);
assert.match(podspec, /s\.dependency 'NativeBindings'/);
assert.match(podspec, /generated\/PrnsAppBindingsFFI\.h/);
assert.doesNotMatch(podspec, /-lprns_app|libprns_app\.a|s\.script_phase/);
assert.match(bindingsPod, /s\.vendored_frameworks = "ios\/prns_app\.xcframework"/);
assert.match(bindingsPod, /s\.dependency "UbjsReactNative"/);
assert.match(podspec, /'UIKit'/, "the lifecycle subscriber must link UIKit explicitly");
assert.match(podspec, /'AccessorySetupKit'/, "the native module must link ASK explicitly");
assert.match(podspec, /:ios => '18\.0'/, "the ASK native module must require iOS 18.0");
assert.match(
  iosDeploymentPlugin,
  /withPodfileProperties[\s\S]*?\["ios\.deploymentTarget"\] = deploymentTarget/,
  "CNG must raise the CocoaPods platform before native-module autolinking",
);
assert.match(
  swift,
  /PRNS_IOS_NATIVE_SMOKE_OK contract=.*starts=2 snapshots=5 stops=2 cleanup=reset/,
  "the named simulator gate must exercise the real native lifecycle and cleanup",
);
assert.equal(
  applicationsPackage.scripts["native:ios:device"],
  "bash sdk/expo/ios/build-development-client.sh --device",
  "the physical development-client helper must be registered explicitly",
);
assert.match(
  developmentClient,
  /\[\[ -n "\$\{DEVICE_ID\}" \]\] \|\| fail "PRNS_IOS_DEVICE_UDID is required with --device"/,
  "physical builds must require an explicit device identifier",
);
assert.match(
  developmentClient,
  /\[\[ -n "\$\{DEVELOPMENT_TEAM\}" \]\] \|\| fail "PRNS_IOS_DEVELOPMENT_TEAM is required with --device"/,
  "physical builds must require an explicit development team",
);
assert.match(
  developmentClient,
  /DESTINATION="platform=iOS,id=\$\{DEVICE_ID\}"/,
  "physical builds must select only the requested device",
);
assert.match(
  developmentClient,
  /"DEVELOPMENT_TEAM=\$\{DEVELOPMENT_TEAM\}"[\s\S]*"RCT_METRO_PORT=\$\{METRO_PORT\}"/,
  "physical builds must pass signing ownership and the validated Metro port to xcodebuild",
);
assert.match(
  developmentClient,
  /else[\s\S]*SIGNING_ARGUMENTS=\([\s\S]*-allowProvisioningUpdates[\s\S]*CODE_SIGN_STYLE=Automatic[\s\S]*\)/,
  "automatic device signing must allow Xcode to update the development profile",
);
assert.match(
  developmentClient,
  /if \[\[ -n "\$\{EXPECTED_PROVISIONING_PROFILE_UUID\}" \]\]; then[\s\S]*SIGNING_ARGUMENTS=\([\s\S]*CODE_SIGN_STYLE=Automatic[\s\S]*"CODE_SIGN_IDENTITY=Apple Development"[\s\S]*\)/,
  "an expected installed profile must use automatic signing without portal permission",
);
assert.match(
  developmentClient,
  /PRNS_IOS_EXPECTED_PROVISIONING_PROFILE_UUID must not be empty[\s\S]*PRNS_IOS_EXPECTED_PROVISIONING_PROFILE_UUID must be a UUID/,
  "installed-profile-only signing must require a nonempty UUID",
);
assert.match(
  developmentClient,
  /"\$\{SIGNING_ARGUMENTS\[@\]\}"[\s\S]*"RCT_METRO_PORT=\$\{METRO_PORT\}"/,
  "physical builds must pass the selected signing argument set as quoted array entries",
);
assert.equal(
  developmentClient.match(/-allowProvisioningUpdates/g)?.length,
  1,
  "only the default automatic signing branch may permit developer-portal updates",
);
assert.match(
  developmentClient,
  /security cms -D -i "\$\{EMBEDDED_PROFILE\}"[\s\S]*plutil -extract UUID raw -[\s\S]*ACTUAL_PROVISIONING_PROFILE_UUID[\s\S]*EXPECTED_PROVISIONING_PROFILE_UUID/,
  "installed-profile-only builds must verify the embedded profile UUID before installation",
);
assert.match(
  developmentClient,
  /PRNS_POD_EXECUTABLE[\s\S]*?--version >\/dev\/null 2>&1[\s\S]*?fail "PRNS_POD_EXECUTABLE cannot run/,
  "an explicit CocoaPods executable must be health-checked before project generation",
);
assert.match(
  developmentClient,
  /for candidate in "\$\{POD_ON_PATH\}" \/opt\/homebrew\/bin\/pod \/usr\/local\/bin\/pod; do[\s\S]*?"\$\{candidate\}" --version >\/dev\/null 2>&1[\s\S]*?POD_EXECUTABLE="\$\{candidate\}"/,
  "automatic CocoaPods selection must skip broken PATH shims and try standard installations",
);
assert.match(
  developmentClient,
  /\[\[ "\$\{METRO_PORT\}" =~ \^\[0-9\]\+\$ \]\] \|\| fail "PRNS_IOS_METRO_PORT must be an integer"/,
  "development-client builds must validate the Metro port syntax",
);
assert.match(
  developmentClient,
  /METRO_PORT >= 1024 && METRO_PORT <= 65535/,
  "development-client builds must validate the Metro port range",
);
assert.equal(
  developmentClient.match(/assert_development_client_metadata "\$\{APP_BUNDLE\}"/g)?.length,
  2,
  "simulator and physical builds must both validate compiled bundle metadata",
);
assert.match(
  developmentClient,
  /NSAccessorySetupKitSupports\.0/,
  "CNG and built-app checks must validate the physically proven ASK support key",
);
assert.doesNotMatch(developmentClient, /NSAccessorySetupSupports\.0/);
assert.match(
  configCheck,
  /if \("UIApplicationSceneManifest" in infoPlist\) \{\s*fail\(`\$\{variant\}\.ios\.infoPlist must not declare a scene manifest`\);\s*\}/,
  "both rendered app variants must leave restoration launch options with AppDelegate",
);
assert.equal(
  developmentClient.match(/plutil -extract UIApplicationSceneManifest raw/g)?.length,
  2,
  "clean CNG and the built app must both reject a scene manifest",
);
assert.match(
  developmentClient,
  /UIApplicationSceneManifest raw "\$\{INFO_PLIST\}"[\s\S]*?clean CNG rendered a scene manifest[\s\S]*?UIApplicationSceneManifest raw "\$\{built_info_plist\}"[\s\S]*?development client has a scene manifest/,
);
assert.match(developmentClient, /MinimumOSVersion[\s\S]*?18\.0/);
assert.match(developmentClient, /PODFILE_PROPERTIES[\s\S]*?ios\.deploymentTarget[\s\S]*?18\.0/);
assert.match(
  developmentClient,
  /plutil -extract CFBundleIdentifier raw "\$\{built_info_plist\}"/,
  "compiled development clients must contain the expected bundle identifier",
);
assert.match(
  developmentClient,
  /plutil -extract RCTMetroPort raw "\$\{built_info_plist\}"/,
  "compiled development clients must contain the requested Metro port",
);
assert.match(
  developmentClient,
  /\[\[ -f "\$\{PACKAGER_IP_FILE\}" \]\] \|\| fail "development client does not contain ip\.txt"/,
  "physical installation must require a generated packager-host file",
);
assert.match(
  developmentClient,
  /\[\[ -n "\$\{PACKAGER_HOST\/\/\[\[:space:\]\]\/\}" \]\] \|\| fail "development client contains an empty ip\.txt"/,
  "physical installation must reject an empty packager host",
);
assert.match(
  developmentClient,
  /xcrun devicectl device install app --device "\$\{DEVICE_ID\}" "\$\{APP_BUNDLE\}"/,
  "physical installation must target only the requested device",
);

const recoveryTestDirectory = mkdtempSync(resolve(tmpdir(), "prns-protected-data-recovery-"));
try {
  const recoveryTestExecutable = resolve(recoveryTestDirectory, "recovery-tests");
  execFileSync(
    "xcrun",
    [
      "swiftc",
      resolve(packageRoot, "ios/PrnsProtectedDataRecovery.swift"),
      resolve(packageRoot, "ios/PrnsRestorationDispatch.swift"),
      resolve(packageRoot, "scripts/PrnsProtectedDataRecoveryTests.swift"),
      "-o",
      recoveryTestExecutable,
    ],
    { stdio: "inherit" },
  );
  execFileSync(recoveryTestExecutable, [], { stdio: "inherit" });

  const probeSource = resolve(packageRoot, "ios/PrnsAppRestorationProbe.swift");
  const probeTestExecutable = resolve(recoveryTestDirectory, "probe-tests");
  execFileSync(
    "xcrun",
    [
      "swiftc",
      "-D",
      "DEBUG",
      resolve(packageRoot, "ios/PrnsIosDiagnostics.swift"),
      probeSource,
      resolve(packageRoot, "scripts/PrnsAppRestorationProbeTests.swift"),
      "-o",
      probeTestExecutable,
    ],
    { stdio: "inherit" },
  );
  const probeResult = spawnSync(probeTestExecutable, [], { encoding: "utf8" });
  assert.ifError(probeResult.error);
  assert.equal(probeResult.status, 0, probeResult.stderr);
  assert.equal(probeResult.stdout, "");
  const probeTag = "PRNS_IOS_";
  const probeLines = probeResult.stderr
    .split("\n")
    .filter((line) => line.includes(probeTag))
    .map((line) => line.slice(line.indexOf(probeTag)));
  assert.deepEqual(
    probeLines,
    [
      "PRNS_IOS_LIFECYCLE launch centralRestoration=true protectedData=false",
      "PRNS_IOS_ASK phase=ready picker=idle authorized=1 nativeStart=running restoration=true",
      "PRNS_IOS_LIFECYCLE prepare outcome=prepared stage=none",
      "PRNS_IOS_LIFECYCLE start outcome=failed stage=runtime",
      "PRNS_IOS_LIFECYCLE prepare outcome=unknown stage=unknown",
      "PRNS_IOS_RESTORATION sequence=17 event=logger_installed",
      "PRNS_IOS_RESTORATION sequence=18 event=central_scan_already_scanning",
      "PRNS_IOS_RESTORATION sequence=18446744073709551615 event=central_scan_started",
    ],
    "each diagnostic channel must reach stderr once; invalid probe codes must stay silent",
  );
  assert.doesNotMatch(
    probeResult.stderr,
    /private-peer|private-error|private-outcome|private-stage/,
    "unknown native values and rejected restoration payloads must never become public",
  );
  const releaseProbeObject = resolve(recoveryTestDirectory, "probe-release.o");
  execFileSync(
    "xcrun",
    ["swiftc", "-parse-as-library", "-emit-object", probeSource, "-o", releaseProbeObject],
    { stdio: "inherit" },
  );
  const releaseProbeSymbols = execFileSync("xcrun", ["nm", "-g", releaseProbeObject], {
    encoding: "utf8",
  });
  assert.doesNotMatch(
    releaseProbeSymbols,
    /prns_app_ios_restoration_probe_emit|prnsAppIosRestorationProbeEmit/,
    "non-Debug compilation must omit the diagnostic callback entirely",
  );
  const releaseDiagnosticsObject = resolve(recoveryTestDirectory, "diagnostics-release.o");
  execFileSync(
    "xcrun",
    [
      "swiftc",
      "-parse-as-library",
      "-emit-object",
      resolve(packageRoot, "ios/PrnsIosDiagnostics.swift"),
      "-o",
      releaseDiagnosticsObject,
    ],
    { stdio: "inherit" },
  );
  const releaseDiagnosticsSymbols = execFileSync("xcrun", ["nm", "-g", releaseDiagnosticsObject], {
    encoding: "utf8",
  });
  assert.doesNotMatch(
    releaseDiagnosticsSymbols,
    /\b_fputs\b/,
    "non-Debug diagnostics must omit the stderr mirror",
  );
} finally {
  rmSync(recoveryTestDirectory, { force: true, recursive: true });
}

console.log(
  "ios:check: generated native transport, lifecycle, linkage, and iOS development-client checks passed",
);
