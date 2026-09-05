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
const rustBuild = readFileSync(resolve(packageRoot, "ios/build-rust.sh"), "utf8");
const nativeCargo = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/Cargo.toml"),
  "utf8",
);
const nativeFfi = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/src/ffi.rs"),
  "utf8",
);
const nativeHeader = readFileSync(
  resolve(packageRoot, "../../prns/native-composition/include/prns_app.h"),
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

function normalizedCode(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/\/\/[^\n]*/g, " ")
    .replace(/\s+/g, " ")
    .replace(/\(\s+/g, "(")
    .replace(/\s+\)/g, ")")
    .trim();
}

function assertOne(source, pattern, message) {
  const flags = pattern.flags.includes("g") ? pattern.flags : `${pattern.flags}g`;
  assert.equal([...source.matchAll(new RegExp(pattern.source, flags))].length, 1, message);
}

function quotedLiteralEnd(source, index) {
  let hashCount = 0;
  let quoteIndex = index;
  if (source[index] === "r" || (source[index] === "b" && source[index + 1] === "r")) {
    quoteIndex = index + (source[index] === "b" ? 2 : 1);
    while (source[quoteIndex + hashCount] === "#") hashCount += 1;
    quoteIndex += hashCount;
  } else if (source[index] === "#") {
    while (source[index + hashCount] === "#") hashCount += 1;
    quoteIndex = index + hashCount;
  }
  if (hashCount > 0 || quoteIndex !== index) {
    if (source[quoteIndex] !== '"') return null;
    const quote = source.startsWith('"""', quoteIndex) ? '"""' : '"';
    const closing = `${quote}${"#".repeat(hashCount)}`;
    const closingIndex = source.indexOf(closing, quoteIndex + quote.length);
    assert.notEqual(closingIndex, -1, "raw string literal must be terminated");
    return closingIndex + closing.length;
  }
  if (source.startsWith('"""', index)) {
    const closingIndex = source.indexOf('"""', index + 3);
    assert.notEqual(closingIndex, -1, "multiline string literal must be terminated");
    return closingIndex + 3;
  }
  if (source[index] === '"') {
    for (let cursor = index + 1; cursor < source.length; cursor += 1) {
      if (source[cursor] === "\\") cursor += 1;
      else if (source[cursor] === '"') return cursor + 1;
    }
    assert.fail("string literal must be terminated");
  }
  if (source[index] === "'") {
    let cursor = index + 1;
    if (source[cursor] === "\\") cursor += 2;
    else cursor += 1;
    return source[cursor] === "'" ? cursor + 1 : null;
  }
  return null;
}

function scopedBody(source, declaration, owner) {
  const flags = declaration.flags.includes("g") ? declaration.flags : `${declaration.flags}g`;
  const matches = [...source.matchAll(new RegExp(declaration.source, flags))];
  assert.equal(matches.length, 1, `${owner} must have exactly one declaration`);
  const opening = source.indexOf("{", (matches[0].index ?? 0) + matches[0][0].length);
  assert.notEqual(opening, -1, `${owner} must have a body`);
  let depth = 0;
  for (let index = opening; index < source.length; index += 1) {
    const literalEnd = quotedLiteralEnd(source, index);
    if (literalEnd !== null) {
      index = literalEnd - 1;
      continue;
    }
    if (source[index] === "{") depth += 1;
    if (source[index] !== "}") continue;
    depth -= 1;
    if (depth === 0) return source.slice(opening + 1, index);
  }
  assert.fail(`${owner} body must have balanced braces`);
}

const braceStringFixture = normalizedCode(`
  func target() {
    let normal = "{"
    let character = '}'
    let swiftRaw = #"{"#
    let rustRaw = r#"}"#
    let multiline = """{"""
    crossedLengths()
  }
  func laterHelper() { canonicalLookingCall() }
`);
const braceStringBody = scopedBody(braceStringFixture, /func target\(\)/, "brace-string fixture");
assert.match(braceStringBody, /crossedLengths\(\)/);
assert.doesNotMatch(braceStringBody, /canonicalLookingCall/);

function assertCentralRestorationAbi(headerSource, rustSource, swiftSource) {
  const header = normalizedCode(headerSource);
  const rust = normalizedCode(rustSource);
  const swiftCode = normalizedCode(swiftSource);
  const prepare = "prns_app_prepare_apple_bluetooth_central_restoration";
  const start = "prns_app_start_with_apple_bluetooth_central_restoration";
  assertOne(
    header,
    /PrnsAppBytes prns_app_prepare_apple_bluetooth_central_restoration\(const uint8_t \*path_ptr, size_t path_len, const uint8_t \*central_identifier_ptr, size_t central_identifier_len\);/,
    "C prepare ABI must have exactly two pointer/size_t pairs",
  );
  assertOne(
    header,
    /PrnsAppBytes prns_app_start_with_apple_bluetooth_central_restoration\(const uint8_t \*path_ptr, size_t path_len, const uint8_t \*input_ptr, size_t input_len, const uint8_t \*central_identifier_ptr, size_t central_identifier_len\);/,
    "C start ABI must have exactly three pointer/size_t pairs",
  );
  assertOne(
    rust,
    /#\[no_mangle\] pub unsafe extern "C" fn prns_app_prepare_apple_bluetooth_central_restoration\(path_ptr: \*const u8, path_len: usize, central_identifier_ptr: \*const u8, central_identifier_len: usize,?\) -> PrnsAppBytes \{/,
    "Rust prepare ABI must match the C pointer/length order and types",
  );
  assertOne(
    rust,
    /#\[no_mangle\] pub unsafe extern "C" fn prns_app_start_with_apple_bluetooth_central_restoration\(path_ptr: \*const u8, path_len: usize, input_ptr: \*const u8, input_len: usize, central_identifier_ptr: \*const u8, central_identifier_len: usize,?\) -> PrnsAppBytes \{/,
    "Rust start ABI must match the C pointer/length order and types",
  );
  const rustPrepareBody = scopedBody(
    rust,
    /pub unsafe extern "C" fn prns_app_prepare_apple_bluetooth_central_restoration\(/,
    "Rust prepare wrapper",
  );
  assert.match(
    rustPrepareBody,
    /invoke_path_string\(path_ptr, path_len, central_identifier_ptr, central_identifier_len, lifecycle::prepare_apple_bluetooth_central_restoration,?\)/,
    "Rust prepare wrapper must forward both pointer/length pairs in ABI order",
  );
  const rustStartBody = scopedBody(
    rust,
    /pub unsafe extern "C" fn prns_app_start_with_apple_bluetooth_central_restoration\(/,
    "Rust restoring start wrapper",
  );
  assert.match(
    rustStartBody,
    /invoke_path_json_string::<DevelopmentNodeStartInput, _, _>\(path_ptr, path_len, input_ptr, input_len, central_identifier_ptr, central_identifier_len, lifecycle::start_configured_with_apple_bluetooth_central_restoration,?\)/,
    "Rust restoring start wrapper must forward all pointer/length pairs in ABI order",
  );
  assertOne(
    swiftCode,
    /private static func withUtf8Bytes<Result>\(_ value: String, operation: \(UnsafePointer<UInt8>\?, Int\) throws -> Result\) rethrows -> Result \{/,
    "Swift UTF-8 binder must provide the imported C pointer/size_t types",
  );
  const swiftPrepareBody = scopedBody(
    swiftCode,
    /static func prepareBluetoothCentralRestoration\(\) throws -> String/,
    "Swift prepare wrapper",
  );
  assert.match(
    swiftPrepareBody,
    /withUtf8Bytes\(storageURL\.path\) \{ pathPointer, pathCount in.*?withUtf8Bytes\(identifier\) \{ centralPointer, centralCount in.*?prns_app_prepare_apple_bluetooth_central_restoration\(pathPointer, pathCount, centralPointer, centralCount\)/,
    "Swift prepare must pass both typed pairs in C ABI order",
  );
  const swiftStartBody = scopedBody(
    swiftCode,
    /static func startWithCentralRestoration\(_ inputJSON: String\) throws -> String/,
    "Swift restoring start wrapper",
  );
  assert.match(
    swiftStartBody,
    /withUtf8Bytes\(storageURL\.path\) \{ pathPointer, pathCount in.*?withUtf8Bytes\(inputJSON\) \{ inputPointer, inputCount in.*?withUtf8Bytes\(identifier\) \{ centralPointer, centralCount in.*?prns_app_start_with_apple_bluetooth_central_restoration\(pathPointer, pathCount, inputPointer, inputCount, centralPointer, centralCount\)/,
    "Swift start must pass all three typed pairs in C ABI order",
  );
  const swiftSmokeBody = scopedBody(
    swiftCode,
    /private static func restoringSmokeStart\(_ pathPointer: UnsafePointer<UInt8>\?, _ pathCount: Int, _ inputPointer: UnsafePointer<UInt8>\?, _ inputCount: Int\) -> PrnsAppBytes/,
    "Swift restoration smoke wrapper",
  );
  assert.match(
    swiftSmokeBody,
    /withUtf8Bytes\(central\) \{ centralPointer, centralCount in prns_app_start_with_apple_bluetooth_central_restoration\(pathPointer, pathCount, inputPointer, inputCount, centralPointer, centralCount\)/,
    "Swift smoke start must pass the same imported types and ABI order",
  );
  assert.equal(swiftCode.match(new RegExp(`\\b${prepare}\\(`, "g"))?.length, 1);
  assert.equal(swiftCode.match(new RegExp(`\\b${start}\\(`, "g"))?.length, 2);
}

function captureOne(source, pattern, owner) {
  const matches = [...source.matchAll(pattern)];
  assert.equal(matches.length, 1, `${owner} must declare exactly one Bluetooth service UUID`);
  return matches[0][1];
}

assertCentralRestorationAbi(nativeHeader, nativeFfi, swift);
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
  29,
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
  /claimIfAuthorized\(nativeStartAuthorized\)[\s\S]*?restorationReady\(\)[\s\S]*?requireAuthorized\(\)[\s\S]*?prepareBluetoothCentralRestoration\(\)[\s\S]*?case "prepared", "alreadyPrepared":[\s\S]*?startNativeRuntime/,
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
  /AsyncFunction\("stop"\)[\s\S]*?beginNativeStop\(\)[\s\S]*?prns_app_stop\(\)[\s\S]*?finishNativeStop\(outcome\)/,
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
assert.match(restorationProbe, /^#if DEBUG\nimport Foundation/);
assert.match(restorationProbe, /\n#endif\s*$/);
assert.match(
  restorationProbe,
  /@_cdecl\("prns_app_ios_restoration_probe_emit"\)/,
  "the private Rust callback must terminate in the Debug-only console-visible sink",
);
assert.match(
  restorationProbe,
  /NSLog\("PRNS_IOS_RESTORATION sequence=%llu event=%@", sequence, code\)/,
);
assert.equal(restorationProbe.match(/\bNSLog\(/g)?.length, 1);
assert.doesNotMatch(
  restorationProbe,
  /\b(?:Logger|os_log)\s*\(/,
  "the restoration sink must not duplicate console events in a second unified-log sink",
);
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
  "prns_app_announce_target",
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
  iosDeploymentPlugin,
  /withPodfileProperties[\s\S]*?\["ios\.deploymentTarget"\] = deploymentTarget/,
  "CNG must raise the CocoaPods platform before native-module autolinking",
);
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
  const probeTag = "PRNS_IOS_RESTORATION ";
  const probeLines = probeResult.stderr
    .split("\n")
    .filter((line) => line.includes(probeTag))
    .map((line) => line.slice(line.indexOf(probeTag)));
  assert.deepEqual(
    probeLines,
    [
      "PRNS_IOS_RESTORATION sequence=17 event=logger_installed",
      "PRNS_IOS_RESTORATION sequence=18 event=central_scan_already_scanning",
      "PRNS_IOS_RESTORATION sequence=18446744073709551615 event=central_scan_started",
    ],
    "each valid probe must reach stderr once; invalid codes must stay silent",
  );
  assert.doesNotMatch(probeResult.stderr, /private-peer|private-error/);
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
} finally {
  rmSync(recoveryTestDirectory, { force: true, recursive: true });
}

console.log(
  "ios:check: native bridge, lifecycle, linkage, and iOS development-client contracts are exact",
);
