import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sdkRoot = resolve(packageRoot, "../../../prns-react-native");
const swift = readFileSync(resolve(packageRoot, "ios/PrnsAppModule.swift"), "utf8");
const coordinator = readFileSync(
  resolve(packageRoot, "ios/PrnsAppLifecycleCoordinator.swift"),
  "utf8",
);
const protectedDataRecovery = readFileSync(
  resolve(sdkRoot, "ios/PrnsProtectedDataRecovery.swift"),
  "utf8",
);
const restorationDispatch = readFileSync(
  resolve(sdkRoot, "ios/PrnsRestorationDispatch.swift"),
  "utf8",
);
const nativeStartDispatch = readFileSync(
  resolve(sdkRoot, "ios/PrnsNativeStartDispatch.swift"),
  "utf8",
);
const bluetoothRuntime = readFileSync(
  resolve(sdkRoot, "ios/PrnsBluetoothRuntimeCoordinator.swift"),
  "utf8",
);
const protectedDataCoordinator = readFileSync(
  resolve(sdkRoot, "ios/PrnsProtectedDataCoordinator.swift"),
  "utf8",
);
const bluetoothCoordinator =
  bluetoothRuntime +
  "\n" +
  readFileSync(resolve(packageRoot, "ios/PrnsBluetoothCoordinator.swift"), "utf8");
const bluetoothAuthorization = readFileSync(
  resolve(sdkRoot, "ios/PrnsBluetoothAuthorization.swift"),
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
  resolve(packageRoot, "../native-composition/bindings/NativeBindings.podspec"),
  "utf8",
);
const sdkPod = readFileSync(resolve(sdkRoot, "ios/PrnsHostExpo.podspec"), "utf8");
const nativeCargo = readFileSync(resolve(packageRoot, "../native-composition/Cargo.toml"), "utf8");
const nativeBindings = readFileSync(
  resolve(packageRoot, "../native-composition/src/bindings/mod.rs"),
  "utf8",
);
const nativeLifecycle = readFileSync(
  resolve(packageRoot, "../native-composition/src/lifecycle.rs"),
  "utf8",
);
const nativeRestorationProbe = readFileSync(
  resolve(packageRoot, "../native-composition/src/ios_restoration_probe.rs"),
  "utf8",
);
const applicationsPackage = JSON.parse(
  readFileSync(resolve(packageRoot, "../../package.json"), "utf8"),
);
const compatibility = JSON.parse(
  readFileSync(resolve(packageRoot, "../../release/compatibility.json"), "utf8"),
);
const appConfig = readFileSync(resolve(packageRoot, "../../prns/app/app.config.ts"), "utf8");
const iosRuntimeProvider = readFileSync(
  resolve(packageRoot, "../../prns/app/src/native/runtime-provider.ios.ts"),
  "utf8",
);
const configCheck = readFileSync(
  resolve(packageRoot, "../../prns/app/tools/config-check.ts"),
  "utf8",
);

function assertOne(source, pattern, message) {
  const flags = pattern.flags.includes("g") ? pattern.flags : `${pattern.flags}g`;
  assert.equal([...source.matchAll(new RegExp(pattern.source, flags))].length, 1, message);
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
assert.match(nativeStartDispatch, /queue\.async\(flags: \.barrier\)/);
assert.match(
  nativeStartDispatch,
  /DispatchQueue\.main\.sync \{ try validate\(\) \}[\s\S]*?if let prepare \{\s*try prepare\(\)\s*try DispatchQueue\.main\.sync \{ try validate\(\) \}[\s\S]*?return try start\(\)/,
  "native restoration must prepare off-main and revalidate admission before starting",
);
assert.match(
  coordinator,
  /PrnsBluetoothCoordinator\.shared\.activate\([\s\S]*?restorationAttemptRequested: restorationAttempt[\s\S]*?prepareAndStartNativeRuntime/,
  "every launch must check app Bluetooth authorization before automatic restoration",
);
assert.match(
  `${bluetoothCoordinator}\n${coordinator}`,
  /claimIfAuthorized\(authorization\.permitsAutomaticRestoration\)[\s\S]*?restorationReady\(\)[\s\S]*?requireRestorationAuthorized\(\)[\s\S]*?startNativeRuntime\(application: application\)/,
  "automatic restoration must require an existing app-wide Bluetooth grant, not a paired accessory",
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
  /AutoBle::prepare_without_restoration[\s\S]*?AutoBle::unavailable_without_restoration/,
  "an unprepared iOS start must expose the ordinary dual-role backend with an offline fallback",
);
assert.doesNotMatch(
  iosPreparedBluetooth,
  /central_only/,
  "the iOS path must not retain the accessory-only central backend",
);
assert.match(
  coordinator,
  /func launch\(application: UIApplication\)[\s\S]*?try PrnsAppModule\.restorationIdentifiers\(\)[\s\S]*?restorationAttempt = true[\s\S]*?catch[\s\S]*?restorationAttempt = false/,
  "every process launch must validate stable central and peripheral identifiers before restoration",
);
assert.match(
  coordinator,
  /\.launch\(\s*restorationAttempt: restorationAttempt/,
  "launch diagnostics must describe an attempt, not a Bluetooth-proven launch reason",
);
assert.doesNotMatch(
  coordinator,
  /\.bluetoothCentrals|\.bluetoothPeripherals|restorationLaunchIdentifiers|UIApplication\.LaunchOptionsKey/,
  "scene-based restoration must not depend on the absent UIKit launch-options dictionary",
);
assert.match(
  nativeLifecycle,
  /fn apple_bluetooth_restoration_storage\([\s\S]*?std::fs::metadata\(storage_root\)[\s\S]*?metadata\.is_dir\(\)[\s\S]*?ErrorKind::NotFound[\s\S]*?prepare_storage\(storage_root\)[\s\S]*?inspect_identity_locked\(state, storage_root\)[\s\S]*?PrimaryIdentityState::Present \{ \.\. \} => return Ok\(paths\)/,
  "automatic preparation must require an existing private directory and primary identity",
);
assert.match(
  nativeLifecycle,
  /fn prepare_apple_bluetooth_restoration_with_supervisor\([\s\S]*?supervisor\.lock_state\(\)[\s\S]*?apple_bluetooth_restoration_storage\(&mut state, storage_root\)[\s\S]*?Err\(outcome\) => return outcome[\s\S]*?load_or_create_ble_identity[\s\S]*?AutoBle::prepare_with_restoration/,
  "the existing-identity gate must run under lifecycle admission before creating a Bluetooth owner",
);
assert.match(
  protectedDataCoordinator,
  /private func waitForProtectedData\(application: UIApplication\)[\s\S]*?addObserver\([\s\S]*?guard !application\.isProtectedDataAvailable else \{[\s\S]*?stopWaiting\(application: application\)[\s\S]*?retry\(application\)/,
  "protected-data waiting must register before rechecking and retry immediately after a lost wake",
);
assert.match(
  coordinator,
  /private func prepareAndStartNativeRuntime\(application: UIApplication\) \{\s+do \{\s+try PrnsBluetoothCoordinator\.shared\.requireRestorationAuthorized\(\)[\s\S]*?protectedData\.beginAttempt\([\s\S]*?startNativeRuntime\(application: application\)/,
  "every initial and resumed automatic attempt must revalidate app authorization before admission",
);
assert.match(
  coordinator,
  /startNative\(\s*input,\s*restorationOnly: true,\s*prepareRestoration: Self\.prepareRestoration/,
  "restoration preparation must use the generation-admitted native startup queue",
);
assert.match(
  swift,
  /requestNativeStart[\s\S]*?PrnsNativeStartDispatch\.enqueue\([\s\S]*?prepare: prepareRestoration[\s\S]*?start: \{ try startWithBluetoothRestoration/,
  "restoration and ordinary startup must share one generation and native owner",
);
assert.doesNotMatch(
  coordinator,
  /private func prepareAndStartNativeRuntime\(application: UIApplication\) \{\s+guard application\.isProtectedDataAvailable/,
  "ordinary after-first-unlock relocks must still attempt restoration against accessible storage",
);
assert.match(
  protectedDataRecovery,
  /action\(recoveryEligible eligible: Bool\)[\s\S]*?let observedUnavailable = unavailableObserved[\s\S]*?guard !consumed, eligible, observedUnavailable[\s\S]*?consumed = true/,
  "protected-data recovery must require observed unavailability and consume one bounded retry",
);
assert.match(
  coordinator,
  /stage == "storage" \|\| stage == "identity" \|\| stage == "persistenceRestore"/,
  "only storage-adjacent native failures may trigger protected-data recovery",
);
assert.match(
  coordinator,
  /recoverAfterProtectedDataFailure\([\s\S]*?requireRestorationAuthorized\(\)[\s\S]*?restorationStartAuthorizationDidClose\(\)[\s\S]*?protectedData\.recoverIfNeeded\(eligible: failure\.eligible, application: application\)/,
  "a recovery must revalidate app authorization and enter the race-safe protected-data wait",
);
assert.match(
  `${bluetoothCoordinator}\n${restorationDispatch}`,
  /restorationStartAuthorizationDidClose\(\)[\s\S]*?rearmAfterAuthorizationLoss\(\)[\s\S]*?claimIfAuthorized[\s\S]*?guard attemptRequested, authorized, !claimed/,
  "authorization loss must re-arm exactly one later restoration dispatch",
);
assert.match(swift, /\.appendingPathComponent\("prns", isDirectory: true\)/);
assert.match(swift, /\.appendingPathComponent\("development", isDirectory: true\)/);
assert.match(
  swift,
  /private static func restorationStorageURL\(\)[\s\S]*?storageURL\(create: false, migrateExistingContents: false\)/,
  "early restoration must neither create a fresh private root nor recursively migrate storage",
);
assert.match(
  swift,
  /static func prepareBluetoothRestoration\(\)[\s\S]*?nativePrepareAppleBluetoothRestoration/,
  "the restoration hook must call the dual-role preparation ABI",
);
assert.match(swift, /FileProtectionType\.completeUntilFirstUserAuthentication/);
assert.match(swift, /isExcludedFromBackup = true/);
assert.match(swift, /isSymbolicLink == true[\s\S]*skipDescendants\(\)/);
assert.deepEqual(moduleConfig.apple?.appDelegateSubscribers, ["PrnsAppDelegateSubscriber"]);
assert.match(subscriber, /willFinishLaunchingWithOptions/);
assertOne(
  subscriber,
  /PrnsAppLifecycleCoordinator\.shared\.launch\(application: application\)/,
  "the AppDelegate subscriber must dispatch one process-owned launch before any scene or JavaScript",
);
assert.doesNotMatch(
  `${swift}\n${coordinator}\n${bluetoothCoordinator}`,
  /CB(?:Central|Peripheral)Manager\s*\(|ASAccessorySession|AccessorySetupKit/,
  "only the Rust owner creates managers; Swift must not create a permission probe or ASK session",
);
assert.match(
  bluetoothCoordinator,
  /switch CBManager\.authorization/,
  "status must read the ordinary app-wide authorization without creating a manager",
);
assert.match(
  bluetoothAuthorization,
  /permitsAutomaticRestoration: Bool \{ self == \.allowedAlways \}/,
  "automatic launch must not ask for Bluetooth permission",
);
assert.match(
  bluetoothAuthorization,
  /return self != \.notDetermined \|\| applicationActive/,
  "ordinary startup must allow an offline node while requiring foreground for a new permission prompt",
);
assert.match(
  bluetoothCoordinator,
  /UIApplication\.didBecomeActiveNotification[\s\S]*?func applicationDidBecomeActive[\s\S]*?refreshAuthorization\(\)/,
  "authorization changed in Settings must refresh independently of JavaScript",
);
assert.match(
  bluetoothCoordinator,
  /maximumWaiters: Int = 8[\s\S]*?nativeStartCompletions\.count < maximumWaiters/,
  "duplicate native starts must be coalesced with a bounded waiter set",
);
assert.match(
  `${swift}\n${bluetoothCoordinator}`,
  /requestNativeStart[\s\S]*?startGeneration[\s\S]*?requireStartAllowed\([\s\S]*?startGeneration:[\s\S]*?restorationOnly:[\s\S]*?startWithBluetoothRestoration/,
  "the app-owned gate must revalidate admission and start ownership before entering Rust",
);
assert.match(
  swift,
  /AsyncFunction\("stop"\)[\s\S]*?beginNativeStop\(\)[\s\S]*?nativeStop\(\)[\s\S]*?finishNativeStop\(outcome\)/,
  "an explicit stop must invalidate the cached native start generation",
);
assert.match(
  `${swift}\n${bluetoothCoordinator}`,
  /beginNativeStop\(\)[\s\S]*?nativeStopWillBegin\(application: \.shared\)[\s\S]*?nativeStopWillBegin\(\)[\s\S]*?restorationDispatch\.cancel\(\)/,
  "explicit stop must cancel both pending lifecycle recovery and restoration dispatch",
);
assert.match(
  coordinator,
  /case \.failure\(let error\):[\s\S]*?cancelsRestoration == true \? \.cancelled : \.bridge[\s\S]*?if case \.cancelled = failure[\s\S]*?finishAttempt\(\)[\s\S]*?return/,
  "stop supersession must terminate before authorization re-arm or protected-data retry",
);
assert.match(
  swift,
  /AsyncFunction\("reset"\)[\s\S]*?beginNativeStop\(\)[\s\S]*?nativeReset[\s\S]*?finishNativeStop\(outcome\)/,
  "reset must invalidate the cached generation before a later onboarding start",
);
assert.match(
  bluetoothCoordinator,
  /nativeStartPhase = joined \? \.notRequested : \.stopping[\s\S]*?case \.alreadyStopped, \.stopped: runtime\.nativeStopDidFinish\(joined: true\)[\s\S]*?default: runtime\.nativeStopDidFinish\(joined: false\)/,
  "only a definitive stopped outcome may reopen native start admission",
);
assert.match(
  bluetoothCoordinator,
  /statusRevision &\+= 1[\s\S]*?onStatus\(status\)[\s\S]*?statusJSON\(\)/,
);
assert.match(
  swift,
  /static func configuredStartInput\(\)[\s\S]*?#if DEBUG\s+let target = Bundle\.main\.object\(forInfoDictionaryKey: "PRNSDevelopmentTcpTarget"\) as\? String\s+return DevelopmentNodeStartInput\(developmentTcpTarget: target\)\s+#else\s+DevelopmentNodeStartInput\(developmentTcpTarget: nil\)\s+#endif/,
  "native startup must use the compiled Debug peer and omit it in Release",
);
assert.match(
  swift,
  /AsyncFunction\("start"\)[\s\S]*?Self\.validatedStartInput\(Self\.decodeStartInput\(inputBytes\)\)[\s\S]*?Self\.startNative\(input\)/,
  "JavaScript starts must resolve the compiled input before native start admission",
);
assert.match(
  swift,
  /private static func validatedStartInput\([\s\S]*?let configured = configuredStartInput\(\)\s+if let requested = input\.developmentTcpTarget,\s+requested != configured\.developmentTcpTarget[\s\S]*?throw PrnsAppException[\s\S]*?return configured/,
  "nil callers must use native configuration and conflicting explicit peers must be rejected",
);
assert.doesNotMatch(
  iosRuntimeProvider,
  /process\.env|EXPO_PUBLIC_PRNS_LXMF_TCP_TARGET/,
  "iOS JavaScript must not select a test peer after process-owned native startup",
);
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
  `${restorationProbe}\n${coordinator}\n${bluetoothCoordinator}\n${iosDiagnostics}`,
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
  /static func bluetoothAuthorization\(\s*authorization: PrnsBluetoothAuthorization,[\s\S]*?restorationAttempt: Bool/,
);
assert.match(
  iosDiagnostics,
  /static func restoration\(sequence: UInt64, event: RestorationEvent\)/,
);
assert.doesNotMatch(
  `${restorationProbe}\n${coordinator}\n${bluetoothCoordinator}`,
  /PrnsIosDiagnostics\.emit/,
  "callers must use typed diagnostic APIs rather than publish arbitrary strings",
);
assert.match(coordinator, /PrnsIosDiagnostics\.lifecycle\(event\)/);
assert.match(bluetoothCoordinator, /PrnsIosDiagnostics\.bluetoothAuthorization\(/);
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
  /nativeStartWithAppleBluetoothRestoration\([\s\S]*?storageRoot:[\s\S]*?input:[\s\S]*?centralIdentifier:[\s\S]*?peripheralIdentifier:/,
);
assert.match(
  swift,
  /nativePrepareAppleBluetoothRestoration\([\s\S]*?storageRoot:[\s\S]*?centralIdentifier:[\s\S]*?peripheralIdentifier:/,
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
assert.doesNotMatch(bindingsPod, /s\.vendored_frameworks/);
assert.match(bindingsPod, /s\.dependency "PrnsHostExpo"/);
assert.match(sdkPod, /s\.vendored_frameworks = "#\{image\}\.xcframework"/);
assert.match(sdkPod, /frameworks == \[expected\]/, "SDK packaging must reject extra native images");
assert.match(bindingsPod, /s\.dependency "UbjsReactNative"/);
assert.match(podspec, /'UIKit'/, "the lifecycle subscriber must link UIKit explicitly");
assert.doesNotMatch(podspec, /'AccessorySetupKit'/, "ordinary Bluetooth must not link ASK");
assert.match(
  podspec,
  /:ios => '18\.0'/,
  "this migration must preserve the iOS 18.0 deployment floor",
);
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
  "bash prns/platform/ios/build-development-client.sh --device",
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
assert.match(developmentClient, /assert_bluetooth_metadata "\$\{INFO_PLIST\}"/);
assert.match(developmentClient, /assert_bluetooth_metadata "\$\{built_info_plist\}"/);
const bluetoothMetadataValidator = developmentClient.match(
  /assert_bluetooth_metadata\(\) \{[\s\S]*?node -e '([\s\S]*?)'/,
)?.[1];
assert.ok(bluetoothMetadataValidator, "the build helper must validate ordinary Bluetooth metadata");
const centralIdentifier = "rs.reticulum.prns.dev.bluetooth-auto.central.v1";
const peripheralIdentifier = "rs.reticulum.prns.dev.bluetooth-auto.peripheral.v1";
const validBluetoothMetadata = {
  UIBackgroundModes: ["bluetooth-central", "bluetooth-peripheral"],
  PRNSCoreBluetoothCentralRestorationIdentifier: centralIdentifier,
  PRNSCoreBluetoothPeripheralRestorationIdentifier: peripheralIdentifier,
};
for (const [name, metadata, valid] of [
  ["dual-role ordinary Bluetooth", validBluetoothMetadata, true],
  ["missing Bluetooth metadata", {}, false],
  [
    "central-only background",
    { ...validBluetoothMetadata, UIBackgroundModes: ["bluetooth-central"] },
    false,
  ],
  [
    "extra background mode",
    {
      ...validBluetoothMetadata,
      UIBackgroundModes: [...validBluetoothMetadata.UIBackgroundModes, "audio"],
    },
    false,
  ],
  [
    "wrong peripheral identifier",
    {
      ...validBluetoothMetadata,
      PRNSCoreBluetoothPeripheralRestorationIdentifier: centralIdentifier,
    },
    false,
  ],
  [
    "missing peripheral identifier",
    { ...validBluetoothMetadata, PRNSCoreBluetoothPeripheralRestorationIdentifier: undefined },
    false,
  ],
  [
    "ASK support declared",
    { ...validBluetoothMetadata, NSAccessorySetupKitSupports: ["Bluetooth"] },
    false,
  ],
  [
    "legacy ASK support declared",
    { ...validBluetoothMetadata, NSAccessorySetupSupports: ["Bluetooth"] },
    false,
  ],
  [
    "ASK discovery filter declared",
    { ...validBluetoothMetadata, NSAccessorySetupBluetoothServices: [bluetoothServiceUuid] },
    false,
  ],
]) {
  const result = spawnSync(
    process.execPath,
    ["-e", bluetoothMetadataValidator, centralIdentifier, peripheralIdentifier],
    {
      input: JSON.stringify(metadata),
      encoding: "utf8",
    },
  );
  assert.ifError(result.error);
  assert.equal(result.status === 0, valid, `Bluetooth metadata guard: ${name}`);
}
assert.match(
  appConfig,
  /"expo-build-properties"[\s\S]*?enableSceneSupport: true/,
  "the app must use Expo's supported scene lifecycle plugin",
);
assert.doesNotMatch(configCheck, /must not declare a scene manifest/);
for (const plist of ["INFO_PLIST", "built_info_plist"]) {
  assert.ok(
    developmentClient.includes(`assert_scene_metadata "\${${plist}}"`),
    "clean CNG and the built app must both validate the single Expo scene",
  );
}
const sceneMetadataValidator = developmentClient.match(
  /assert_scene_metadata\(\) \{[\s\S]*?node -e '([\s\S]*?)'/,
)?.[1];
assert.ok(sceneMetadataValidator, "the build helper must validate the complete scene manifest");
const sceneRole = "UIWindowSceneSessionRoleApplication";
const sceneConfiguration = { UISceneDelegateClassName: "EXExpoAppSceneDelegate" };
const validSceneManifest = {
  UIApplicationSupportsMultipleScenes: false,
  UISceneConfigurations: { [sceneRole]: [sceneConfiguration] },
};
for (const [name, manifest, valid] of [
  ["one Expo scene", validSceneManifest, true],
  ["missing scene metadata", {}, false],
  [
    "multiple scenes enabled",
    { ...validSceneManifest, UIApplicationSupportsMultipleScenes: true },
    false,
  ],
  [
    "missing multiple-scenes policy",
    { UISceneConfigurations: validSceneManifest.UISceneConfigurations },
    false,
  ],
  ["missing application role", { ...validSceneManifest, UISceneConfigurations: {} }, false],
  [
    "an empty scene list",
    { ...validSceneManifest, UISceneConfigurations: { [sceneRole]: [] } },
    false,
  ],
  [
    "two scene configurations",
    {
      ...validSceneManifest,
      UISceneConfigurations: { [sceneRole]: [sceneConfiguration, sceneConfiguration] },
    },
    false,
  ],
  [
    "another scene role",
    {
      ...validSceneManifest,
      UISceneConfigurations: {
        ...validSceneManifest.UISceneConfigurations,
        UIWindowSceneSessionRoleExternalDisplay: [sceneConfiguration],
      },
    },
    false,
  ],
  [
    "a legacy scene delegate",
    {
      ...validSceneManifest,
      UISceneConfigurations: { [sceneRole]: [{ UISceneDelegateClassName: "SceneDelegate" }] },
    },
    false,
  ],
]) {
  const result = spawnSync(process.execPath, ["-e", sceneMetadataValidator], {
    input: JSON.stringify(manifest),
    encoding: "utf8",
  });
  assert.ifError(result.error);
  assert.equal(result.status === 0, valid, `scene metadata guard: ${name}`);
}
assert.match(
  developmentClient,
  /assert_scene_app_delegate\(\)[\s\S]*?ExpoReactNativeFactoryProvider[\s\S]*?assert\.doesNotMatch\(source, \/\\bstartReactNative[\s\S]*?assert\.doesNotMatch\(source, \/\\bUIWindow/,
  "AppDelegate must provide the factory while Expo's scene delegate owns UI startup",
);
assert.match(developmentClient, /assert_scene_app_delegate\n/);
assert.match(
  developmentClient,
  /grep -Fc "PrnsAppDelegateSubscriber\.self"[\s\S]*?== "1"/,
  "generated Expo modules must register the process-owned subscriber exactly once",
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

console.log("ios:check: native ownership, linkage and development-client source checks passed");
