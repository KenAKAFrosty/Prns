import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const swift = readFileSync(resolve(packageRoot, "ios/PrnsAppModule.swift"), "utf8");
const coordinator = readFileSync(
  resolve(packageRoot, "ios/PrnsAppLifecycleCoordinator.swift"),
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
const moduleConfig = JSON.parse(
  readFileSync(resolve(packageRoot, "expo-module.config.json"), "utf8"),
);
const podspec = readFileSync(resolve(packageRoot, "ios/PrnsApp.podspec"), "utf8");
const developmentClient = readFileSync(
  resolve(packageRoot, "ios/build-development-client.sh"),
  "utf8",
);
const rustBuild = readFileSync(resolve(packageRoot, "ios/build-rust.sh"), "utf8");
const nativeCargo = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/Cargo.toml"),
  "utf8",
);
const nativeFfi = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/src/ffi.rs"),
  "utf8",
);
const nativeRestorationProbe = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/src/ios_restoration_probe.rs"),
  "utf8",
);
const applicationsPackage = JSON.parse(
  readFileSync(resolve(packageRoot, "../../package.json"), "utf8"),
);

assert.match(
  swift,
  /DispatchQueue\(\s*label: "rs\.reticulum\.prns\.app\.native",\s*attributes: \.concurrent\s*\)/,
  "native calls must run on a concurrent queue so Rust owns command admission",
);
assert.equal(
  swift.match(/\.runOnQueue\(Self\.nativeQueue\)/g)?.length,
  28,
  "every Expo bridge function must use the native operation queue",
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
  /phase == \.ready,[\s\S]*?restorationLaunchRequested,[\s\S]*?restorationReady\(\)[\s\S]*?prepareBluetoothCentralRestoration\(\)[\s\S]*?case "prepared", "alreadyPrepared":[\s\S]*?startNativeRuntime/,
  "restoration must wait for ASK activation plus an authorized Bluetooth accessory",
);
assert.match(
  coordinator,
  /restorationLaunchIdentifiers[\s\S]*?as\? \[String\][\s\S]*?as\? NSArray[\s\S]*?compactMap/,
  "restoration launch identifiers must accept native Swift arrays and bridged NSArray values",
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
  /static func prepareBluetoothCentralRestoration\(\)[\s\S]*?prns_app_prepare_apple_bluetooth_central_restoration/,
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
  /AsyncFunction\("stop"\)[\s\S]*?beginNativeStop\(\)[\s\S]*?prns_app_stop\(\)[\s\S]*?finishNativeStop\(outcome\)/,
  "an explicit stop must invalidate the cached native start generation",
);
assert.match(
  swift,
  /AsyncFunction\("reset"\)[\s\S]*?beginNativeStop\(\)[\s\S]*?prns_app_reset[\s\S]*?finishNativeStop\(outcome\)/,
  "reset must invalidate the cached generation before a later onboarding start",
);
assert.match(
  accessoryCoordinator,
  /case "alreadyStopped", "stopped":[\s\S]*?nativeStartPhase = \.notRequested[\s\S]*?default:[\s\S]*?nativeStartPhase = \.stopping/,
  "only a definitive stopped outcome may reopen native start admission",
);
assert.match(accessoryCoordinator, /statusRevision &\+= 1[\s\S]*?statusJSON\(\)/);
assert.match(swift, /"developmentTcpTarget": NSNull\(\)/);
assert.doesNotMatch(
  `${coordinator}\n${subscriber}`,
  /applicationDidEnterBackground|applicationWillResignActive|prns_app_stop/,
  "background lifecycle hooks must not stop the native node",
);
assert.match(restorationProbe, /#if DEBUG[\s\S]*?import OSLog/);
assert.match(
  restorationProbe,
  /@_cdecl\("prns_app_ios_restoration_probe_emit"\)/,
  "the private Rust callback must terminate in the Debug-only Apple unified-log sink",
);
assert.match(restorationProbe, /category: "PRNS_IOS_RESTORATION"/);
assert.doesNotMatch(
  `${restorationProbe}\n${nativeRestorationProbe}`,
  /peripheral_service_restored|bluetooth_auto::macos::peripheral/,
  "central-only restoration diagnostics must not claim peripheral-role restoration",
);
assert.match(
  restorationProbe,
  /codeLength > 0, codeLength <= 64[\s\S]*?prnsRestorationEvents\.contains\(code\)/,
  "the restoration sink must accept only bounded allowlisted event codes",
);
assert.match(
  nativeCargo,
  /ios-restoration-probe = \["apple", "dep:log", "prns-interfaces-tokio\/log"\]/,
  "the private restoration logger must remain an explicit app feature",
);
assert.match(
  rustBuild,
  /if \[\[ "\$\{CONFIGURATION:-\}" == "Debug" \]\]; then[\s\S]*?ios-restoration-probe/,
  "only Debug Xcode builds may enable restoration observability",
);
assert.equal(
  nativeFfi.match(/crate::ios_restoration_probe::install\(\);/g)?.length,
  2,
  "both restoration-aware entry points must install the logger idempotently",
);
for (const abiName of [
  "prns_app_prepare_apple_bluetooth_central_restoration",
  "prns_app_start_with_apple_bluetooth_central_restoration",
]) {
  assert.match(
    nativeFfi,
    new RegExp(
      `fn ${abiName}\\([\\s\\S]*?crate::ios_restoration_probe::install\\(\\);[\\s\\S]*?invoke_`,
    ),
    `${abiName} must install restoration logging before constructing a manager`,
  );
}

for (const abiName of [
  "prns_app_contract_fingerprint",
  "prns_app_host_contract_fingerprint",
  "prns_app_inspect_identity",
  "prns_app_preview_identity_import",
  "prns_app_create_generated_identity",
  "prns_app_create_imported_identity",
  "prns_app_prepare_apple_bluetooth_central_restoration",
  "prns_app_start_with_apple_bluetooth_central_restoration",
  "prns_app_snapshot",
  "prns_app_initiate_pairing",
  "prns_app_approve_pairing",
  "prns_app_reject_pairing",
  "prns_app_describe_target",
  "prns_app_save_observed_destination",
  "prns_app_create_manual_contact",
  "prns_app_set_contact_alias",
  "prns_app_set_contact_pinned",
  "prns_app_delete_contact",
  "prns_app_get_contact",
  "prns_app_list_contacts",
  "prns_app_list_lxmf_peers",
  "prns_app_list_lxmf_messages",
  "prns_app_retry_lxmf_message",
  "prns_app_cancel_lxmf_message",
  "prns_app_announce_lxmf",
  "prns_app_measure_lxmf_text",
  "prns_app_send_direct_text",
  "prns_app_stop",
  "prns_app_reset",
  "prns_app_bytes_free",
]) {
  assert.match(swift, new RegExp(`\\b${abiName}\\b`), `Swift bridge must call ${abiName}`);
}

for (const [method, abiName] of [
  ["listLxmfMessages", "prns_app_list_lxmf_messages"],
  ["retryLxmfMessage", "prns_app_retry_lxmf_message"],
  ["cancelLxmfMessage", "prns_app_cancel_lxmf_message"],
]) {
  assert.match(
    swift,
    new RegExp(
      `AsyncFunction\\("${method}"\\)[\\s\\S]*?invokePathJSON\\(inputJSON, operation: ${abiName}\\)`,
    ),
    `${method} must pass the application path with its JSON input`,
  );
}

assert.match(
  podspec,
  /"\$\{PODS_ROOT\}\/\.\.\/\.\.\/\.\.\/native-composition\/include"/,
  "the generated app target must be able to import the public C bridge header",
);
assert.match(podspec, /'UIKit'/, "the lifecycle subscriber must link UIKit explicitly");
assert.match(podspec, /'AccessorySetupKit'/, "the native module must link ASK explicitly");
assert.match(podspec, /:ios => '18\.0'/, "the ASK native module must require iOS 18.0");
assert.match(
  podspec,
  /"\$\{PODS_CONFIGURATION_BUILD_DIR\}"/,
  "the app target and Rust build phase must share one archive directory",
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
assert.match(developmentClient, /MinimumOSVersion[\s\S]*?18\.0/);
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

console.log(
  "ios:check: native bridge, lifecycle, linkage, and iOS development-client contracts are exact",
);
