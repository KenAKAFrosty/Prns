#!/bin/bash
set -euo pipefail

SCRIPT_DIRECTORY="$(cd "$(dirname "$0")" && pwd)"
APPLICATIONS_DIRECTORY="$(cd "${SCRIPT_DIRECTORY}/../../.." && pwd)"
APP_DIRECTORY="${APPLICATIONS_DIRECTORY}/prns/app"
IOS_DIRECTORY="${APP_DIRECTORY}/ios"
EXPO_EXECUTABLE="${APPLICATIONS_DIRECTORY}/node_modules/.bin/expo"
INFO_PLIST="${IOS_DIRECTORY}/prnsdev/Info.plist"
PODFILE_PROPERTIES="${IOS_DIRECTORY}/Podfile.properties.json"
WORKSPACE="${IOS_DIRECTORY}/prnsdev.xcworkspace"
SCHEME="prnsdev"
METRO_PORT="${PRNS_IOS_METRO_PORT:-8088}"
EXPECTED_BUNDLE_IDENTIFIER="rs.reticulum.prns.dev"
BLUETOOTH_USAGE="prns uses Bluetooth to connect to nearby Reticulum nodes."
LOCAL_NETWORK_USAGE="prns uses the local network for an explicitly configured development LXMF peer."
CENTRAL_RESTORATION_IDENTIFIER="rs.reticulum.prns.dev.bluetooth-auto.central.v1"
PRNS_BLUETOOTH_SERVICE="37145B00-442D-4A94-917F-8F42C5DA28E3"

fail() {
  echo "build-development-client.sh: $*" >&2
  exit 1
}

case "${1:-}" in
  "") MODE="simulator" ;;
  --device) MODE="device" ;;
  *) fail "usage: build-development-client.sh [--device]" ;;
esac
(( $# <= 1 )) || fail "usage: build-development-client.sh [--device]"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-development-client.sh: Apple toolchain unavailable; skipping the iOS development client"
  exit 0
fi

DEVICE_ID=""
DEVELOPMENT_TEAM=""
EXPECTED_PROVISIONING_PROFILE_UUID=""
DEFAULT_DERIVED_DATA="${APPLICATIONS_DIRECTORY}/target/ios-development-client"
if [[ "${MODE}" == "device" ]]; then
  DEVICE_ID="${PRNS_IOS_DEVICE_UDID:-}"
  DEVELOPMENT_TEAM="${PRNS_IOS_DEVELOPMENT_TEAM:-}"
  if [[ "${PRNS_IOS_EXPECTED_PROVISIONING_PROFILE_UUID+x}" == x ]]; then
    EXPECTED_PROVISIONING_PROFILE_UUID="${PRNS_IOS_EXPECTED_PROVISIONING_PROFILE_UUID}"
    [[ -n "${EXPECTED_PROVISIONING_PROFILE_UUID}" ]] ||
      fail "PRNS_IOS_EXPECTED_PROVISIONING_PROFILE_UUID must not be empty"
  fi
  DEFAULT_DERIVED_DATA="${APPLICATIONS_DIRECTORY}/target/ios-development-device"
  [[ -n "${DEVICE_ID}" ]] || fail "PRNS_IOS_DEVICE_UDID is required with --device"
  [[ "${DEVICE_ID}" =~ ^[[:alnum:]-]+$ ]] ||
    fail "PRNS_IOS_DEVICE_UDID must contain only letters, numbers, and hyphens"
  [[ -n "${DEVELOPMENT_TEAM}" ]] || fail "PRNS_IOS_DEVELOPMENT_TEAM is required with --device"
  [[ "${DEVELOPMENT_TEAM}" =~ ^[[:alnum:]]+$ ]] ||
    fail "PRNS_IOS_DEVELOPMENT_TEAM must contain only letters and numbers"
  if [[ -n "${EXPECTED_PROVISIONING_PROFILE_UUID}" ]]; then
    [[ "${EXPECTED_PROVISIONING_PROFILE_UUID}" =~ ^[[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12}$ ]] ||
      fail "PRNS_IOS_EXPECTED_PROVISIONING_PROFILE_UUID must be a UUID"
    EXPECTED_PROVISIONING_PROFILE_UUID="$(
      printf '%s' "${EXPECTED_PROVISIONING_PROFILE_UUID}" | tr '[:upper:]' '[:lower:]'
    )"
  fi
fi
DERIVED_DATA="${PRNS_IOS_DERIVED_DATA:-${DEFAULT_DERIVED_DATA}}"

for executable in curl node xcodebuild xcrun plutil security; do
  command -v "${executable}" >/dev/null || fail "${executable} is required"
done
[[ -x "${EXPO_EXECUTABLE}" ]] || fail "run npm ci in ${APPLICATIONS_DIRECTORY} first"
[[ "${METRO_PORT}" =~ ^[0-9]+$ ]] || fail "PRNS_IOS_METRO_PORT must be an integer"
((METRO_PORT >= 1024 && METRO_PORT <= 65535)) ||
  fail "PRNS_IOS_METRO_PORT must be from 1024 through 65535"

POD_EXECUTABLE=""
if [[ -n "${PRNS_POD_EXECUTABLE:-}" ]]; then
  POD_EXECUTABLE="${PRNS_POD_EXECUTABLE}"
  [[ -x "${POD_EXECUTABLE}" ]] ||
    fail "PRNS_POD_EXECUTABLE is not executable: ${POD_EXECUTABLE}"
  env -u LIBRARY_PATH "${POD_EXECUTABLE}" --version >/dev/null 2>&1 ||
    fail "PRNS_POD_EXECUTABLE cannot run: ${POD_EXECUTABLE}"
else
  POD_ON_PATH="$(command -v pod || true)"
  for candidate in "${POD_ON_PATH}" /opt/homebrew/bin/pod /usr/local/bin/pod; do
    [[ -n "${candidate}" && -x "${candidate}" ]] || continue
    if env -u LIBRARY_PATH "${candidate}" --version >/dev/null 2>&1; then
      POD_EXECUTABLE="${candidate}"
      break
    fi
  done
  [[ -n "${POD_EXECUTABLE}" ]] ||
    fail "CocoaPods is required, but no usable pod executable was found"
fi

echo "build-development-client.sh: generating a clean development iOS project"
(
  cd "${APP_DIRECTORY}"
  env CI=1 PRNS_APP_VARIANT=development "${EXPO_EXECUTABLE}" prebuild \
    --clean \
    --platform ios \
    --no-install
)

[[ -f "${INFO_PLIST}" ]] || fail "clean CNG did not render ${INFO_PLIST}"
[[ -f "${PODFILE_PROPERTIES}" ]] || fail "clean CNG did not render ${PODFILE_PROPERTIES}"
[[ "$(node -e 'const value = require(process.argv[1])["ios.deploymentTarget"]; process.stdout.write(String(value ?? ""))' "${PODFILE_PROPERTIES}")" == "18.0" ]] ||
  fail "clean CNG did not render the CocoaPods iOS 18.0 deployment target"
[[ "$(plutil -extract NSBluetoothAlwaysUsageDescription raw "${INFO_PLIST}")" == "${BLUETOOTH_USAGE}" ]] ||
  fail "clean CNG rendered unexpected Bluetooth usage copy"
[[ "$(plutil -extract NSLocalNetworkUsageDescription raw "${INFO_PLIST}")" == "${LOCAL_NETWORK_USAGE}" ]] ||
  fail "clean CNG rendered unexpected local-network usage copy"
[[ "$(plutil -extract UIBackgroundModes.0 raw "${INFO_PLIST}")" == "bluetooth-central" ]] ||
  fail "clean CNG did not render the central Bluetooth background mode"
if plutil -extract UIBackgroundModes.1 raw "${INFO_PLIST}" >/dev/null 2>&1; then
  fail "clean CNG rendered an unexpected second background mode"
fi
[[ "$(plutil -extract PRNSCoreBluetoothCentralRestorationIdentifier raw "${INFO_PLIST}")" == "${CENTRAL_RESTORATION_IDENTIFIER}" ]] ||
  fail "clean CNG rendered the wrong central restoration identifier"
if plutil -extract PRNSCoreBluetoothPeripheralRestorationIdentifier raw "${INFO_PLIST}" >/dev/null 2>&1; then
  fail "clean CNG rendered a peripheral restoration identifier"
fi
[[ "$(plutil -extract NSAccessorySetupKitSupports.0 raw "${INFO_PLIST}")" == "Bluetooth" ]] ||
  fail "clean CNG did not render ASK Bluetooth support"
[[ "$(plutil -extract NSAccessorySetupBluetoothServices.0 raw "${INFO_PLIST}")" == "${PRNS_BLUETOOTH_SERVICE}" ]] ||
  fail "clean CNG rendered the wrong ASK Bluetooth service"
DEPLOYMENT_TARGET_LINES="$(grep -E "IPHONEOS_DEPLOYMENT_TARGET = " "${IOS_DIRECTORY}/prnsdev.xcodeproj/project.pbxproj")"
[[ -n "${DEPLOYMENT_TARGET_LINES}" ]] || fail "clean CNG did not render an iOS deployment target"
if grep -Fv "IPHONEOS_DEPLOYMENT_TARGET = 18.0;" <<<"${DEPLOYMENT_TARGET_LINES}" >/dev/null; then
  fail "clean CNG did not render the exact iOS 18.0 deployment target"
fi

echo "build-development-client.sh: installing CocoaPods dependencies"
(
  cd "${IOS_DIRECTORY}"
  env -u LIBRARY_PATH "${POD_EXECUTABLE}" install
)
[[ -d "${WORKSPACE}" ]] || fail "CocoaPods did not generate ${WORKSPACE}"
grep -Fq "PrnsApp" "${IOS_DIRECTORY}/Podfile.lock" || fail "PrnsApp was not autolinked"
EXPO_MODULES_PROVIDER="${IOS_DIRECTORY}/Pods/Target Support Files/Pods-prnsdev/ExpoModulesProvider.swift"
[[ -f "${EXPO_MODULES_PROVIDER}" ]] || fail "Expo did not generate its modules provider"
[[ "$(grep -Fc "PrnsAppDelegateSubscriber.self" "${EXPO_MODULES_PROVIDER}")" == "1" ]] ||
  fail "Expo must register PrnsAppDelegateSubscriber exactly once"

assert_development_client_metadata() {
  local app_bundle="$1"
  local built_info_plist="${app_bundle}/Info.plist"

  [[ -d "${app_bundle}" ]] || fail "development client was not produced at ${app_bundle}"
  [[ "$(plutil -extract CFBundleIdentifier raw "${built_info_plist}")" == "${EXPECTED_BUNDLE_IDENTIFIER}" ]] ||
    fail "development client has the wrong bundle identifier"
  [[ "$(plutil -extract RCTMetroPort raw "${built_info_plist}")" == "${METRO_PORT}" ]] ||
    fail "development client does not contain the requested Metro port ${METRO_PORT}"
  [[ "$(plutil -extract UIBackgroundModes.0 raw "${built_info_plist}")" == "bluetooth-central" ]] ||
    fail "development client is missing the central Bluetooth background mode"
  if plutil -extract UIBackgroundModes.1 raw "${built_info_plist}" >/dev/null 2>&1; then
    fail "development client has an unexpected second background mode"
  fi
  [[ "$(plutil -extract PRNSCoreBluetoothCentralRestorationIdentifier raw "${built_info_plist}")" == "${CENTRAL_RESTORATION_IDENTIFIER}" ]] ||
    fail "development client has the wrong central restoration identifier"
  if plutil -extract PRNSCoreBluetoothPeripheralRestorationIdentifier raw "${built_info_plist}" >/dev/null 2>&1; then
    fail "development client has a peripheral restoration identifier"
  fi
  [[ "$(plutil -extract NSAccessorySetupKitSupports.0 raw "${built_info_plist}")" == "Bluetooth" ]] ||
    fail "development client is missing ASK Bluetooth support"
  [[ "$(plutil -extract NSAccessorySetupBluetoothServices.0 raw "${built_info_plist}")" == "${PRNS_BLUETOOTH_SERVICE}" ]] ||
    fail "development client has the wrong ASK Bluetooth service"
  [[ "$(plutil -extract MinimumOSVersion raw "${built_info_plist}")" == "18.0" ]] ||
    fail "development client does not require iOS 18.0"
}

if [[ "${MODE}" == "device" ]]; then
  DESTINATION="platform=iOS,id=${DEVICE_ID}"
  if [[ -n "${EXPECTED_PROVISIONING_PROFILE_UUID}" ]]; then
    echo "build-development-client.sh: using installed-profile-only automatic signing"
    SIGNING_ARGUMENTS=(
      CODE_SIGN_STYLE=Automatic
      "DEVELOPMENT_TEAM=${DEVELOPMENT_TEAM}"
      "CODE_SIGN_IDENTITY=Apple Development"
    )
  else
    SIGNING_ARGUMENTS=(
      -allowProvisioningUpdates
      CODE_SIGN_STYLE=Automatic
      "DEVELOPMENT_TEAM=${DEVELOPMENT_TEAM}"
    )
  fi
  echo "build-development-client.sh: building ${SCHEME} for ${DESTINATION}"
  env -u LIBRARY_PATH xcodebuild -quiet \
    -workspace "${WORKSPACE}" \
    -scheme "${SCHEME}" \
    -configuration Debug \
    -destination "${DESTINATION}" \
    -destination-timeout 60 \
    -derivedDataPath "${DERIVED_DATA}" \
    "${SIGNING_ARGUMENTS[@]}" \
    "RCT_METRO_PORT=${METRO_PORT}" \
    build

  APP_BUNDLE="${DERIVED_DATA}/Build/Products/Debug-iphoneos/prnsdev.app"
  assert_development_client_metadata "${APP_BUNDLE}"
  if [[ -n "${EXPECTED_PROVISIONING_PROFILE_UUID}" ]]; then
    EMBEDDED_PROFILE="${APP_BUNDLE}/embedded.mobileprovision"
    [[ -f "${EMBEDDED_PROFILE}" ]] ||
      fail "development client does not contain an embedded provisioning profile"
    ACTUAL_PROVISIONING_PROFILE_UUID="$(
      security cms -D -i "${EMBEDDED_PROFILE}" 2>/dev/null |
        plutil -extract UUID raw - |
        tr '[:upper:]' '[:lower:]'
    )" || fail "development client contains an unreadable provisioning profile"
    [[ "${ACTUAL_PROVISIONING_PROFILE_UUID}" == "${EXPECTED_PROVISIONING_PROFILE_UUID}" ]] ||
      fail "development client used provisioning profile ${ACTUAL_PROVISIONING_PROFILE_UUID}, expected ${EXPECTED_PROVISIONING_PROFILE_UUID}"
  fi
  PACKAGER_IP_FILE="${APP_BUNDLE}/ip.txt"
  [[ -f "${PACKAGER_IP_FILE}" ]] || fail "development client does not contain ip.txt"
  PACKAGER_HOST="$(<"${PACKAGER_IP_FILE}")"
  [[ -n "${PACKAGER_HOST//[[:space:]]/}" ]] || fail "development client contains an empty ip.txt"

  echo "build-development-client.sh: installing on explicitly selected device ${DEVICE_ID}"
  xcrun devicectl device install app --device "${DEVICE_ID}" "${APP_BUNDLE}"
  echo "IOS_DEVELOPMENT_DEVICE_OK app=${APP_BUNDLE} device=${DEVICE_ID} metro_port=${METRO_PORT}"
  exit 0
fi

SIMULATOR_SELECTION="$({
  xcrun simctl list devices available --json |
    PRNS_REQUESTED_SIMULATOR_ID="${PRNS_IOS_SIMULATOR_UDID:-}" node -e '
      let input = "";
      process.stdin.setEncoding("utf8");
      process.stdin.on("data", (chunk) => { input += chunk; });
      process.stdin.on("end", () => {
        const requested = process.env.PRNS_REQUESTED_SIMULATOR_ID;
        const devices = Object.values(JSON.parse(input).devices)
          .flat()
          .filter((device) => device.isAvailable && device.name.startsWith("iPhone "));
        const selected = requested
          ? devices.find((device) => device.udid === requested)
          : devices.find((device) => device.state === "Booted") ?? devices[0];
        if (selected) process.stdout.write(`${selected.udid}\t${selected.name}`);
      });
    '
})"
[[ -n "${SIMULATOR_SELECTION}" ]] || {
  if [[ -n "${PRNS_IOS_SIMULATOR_UDID:-}" ]]; then
    fail "named iPhone simulator ${PRNS_IOS_SIMULATOR_UDID} is not available"
  fi
  fail "no available iPhone simulator was found"
}
IFS=$'\t' read -r SIMULATOR_ID SIMULATOR_NAME <<<"${SIMULATOR_SELECTION}"
DESTINATION="platform=iOS Simulator,id=${SIMULATOR_ID}"

echo "build-development-client.sh: building ${SCHEME} for ${DESTINATION}"
env -u LIBRARY_PATH xcodebuild -quiet \
  -workspace "${WORKSPACE}" \
  -scheme "${SCHEME}" \
  -configuration Debug \
  -destination "${DESTINATION}" \
  -derivedDataPath "${DERIVED_DATA}" \
  CODE_SIGNING_ALLOWED=NO \
  RCT_METRO_PORT="${METRO_PORT}" \
  build

APP_BUNDLE="${DERIVED_DATA}/Build/Products/Debug-iphonesimulator/prnsdev.app"
assert_development_client_metadata "${APP_BUNDLE}"

echo "build-development-client.sh: installing and launching on ${SIMULATOR_NAME} (${SIMULATOR_ID})"
xcrun simctl boot "${SIMULATOR_ID}" 2>/dev/null || true
xcrun simctl bootstatus "${SIMULATOR_ID}" -b
xcrun simctl terminate "${SIMULATOR_ID}" rs.reticulum.prns.dev 2>/dev/null || true
xcrun simctl install "${SIMULATOR_ID}" "${APP_BUNDLE}"

SMOKE_LOG_DIRECTORY="$(mktemp -d "${TMPDIR:-/tmp}/prns-ios-native-smoke.XXXXXX")"
METRO_LOG="${SMOKE_LOG_DIRECTORY}/metro.log"
SMOKE_STDOUT="${SMOKE_LOG_DIRECTORY}/app.stdout"
SMOKE_STDERR="${SMOKE_LOG_DIRECTORY}/app.stderr"
METRO_PID=""
LAUNCH_PID=""
cleanup() {
  if [[ -n "${LAUNCH_PID}" ]]; then
    kill "${LAUNCH_PID}" 2>/dev/null || true
    wait "${LAUNCH_PID}" 2>/dev/null || true
  fi
  if [[ -n "${METRO_PID}" ]]; then
    kill "${METRO_PID}" 2>/dev/null || true
    wait "${METRO_PID}" 2>/dev/null || true
  fi
  rm -f "${METRO_LOG}" "${SMOKE_STDOUT}" "${SMOKE_STDERR}"
  rmdir "${SMOKE_LOG_DIRECTORY}" 2>/dev/null || true
}
trap cleanup EXIT

echo "build-development-client.sh: starting isolated Metro on port ${METRO_PORT}"
(
  cd "${APP_DIRECTORY}"
  exec env CI=1 "${EXPO_EXECUTABLE}" start --localhost --port "${METRO_PORT}"
) >"${METRO_LOG}" 2>&1 &
METRO_PID="$!"
metro_ready=false
for _attempt in {1..30}; do
  if curl --fail --silent "http://localhost:${METRO_PORT}/status" | grep -Fq "packager-status:running"; then
    metro_ready=true
    break
  fi
  if ! kill -0 "${METRO_PID}" 2>/dev/null; then
    tail -80 "${METRO_LOG}" >&2 || true
    fail "isolated Metro stopped before becoming ready"
  fi
  sleep 1
done
if [[ "${metro_ready}" != true ]]; then
  tail -80 "${METRO_LOG}" >&2 || true
  fail "isolated Metro did not become ready"
fi

SIMCTL_CHILD_PRNS_IOS_NATIVE_SMOKE=1 xcrun simctl launch \
  --terminate-running-process \
  --console \
  "${SIMULATOR_ID}" \
  rs.reticulum.prns.dev >"${SMOKE_STDOUT}" 2>"${SMOKE_STDERR}" &
LAUNCH_PID="$!"

smoke_passed=false
for _attempt in {1..30}; do
  if grep -Fq "PRNS_IOS_NATIVE_SMOKE_OK" "${SMOKE_STDOUT}" "${SMOKE_STDERR}" 2>/dev/null; then
    smoke_passed=true
    break
  fi
  if grep -Fq "PRNS_IOS_NATIVE_SMOKE_FAILED" "${SMOKE_STDOUT}" "${SMOKE_STDERR}" 2>/dev/null; then
    break
  fi
  if ! kill -0 "${LAUNCH_PID}" 2>/dev/null; then
    break
  fi
  sleep 1
done
if [[ "${smoke_passed}" != true ]]; then
  tail -80 "${SMOKE_STDOUT}" "${SMOKE_STDERR}" >&2 || true
  fail "the simulator native lifecycle smoke did not pass"
fi

SMOKE_MARKER="$(grep -F "PRNS_IOS_NATIVE_SMOKE_OK" "${SMOKE_STDOUT}" "${SMOKE_STDERR}" | tail -1)"
echo "${SMOKE_MARKER}"
echo "IOS_DEVELOPMENT_CLIENT_OK app=${APP_BUNDLE} simulator=${SIMULATOR_ID} name=${SIMULATOR_NAME}"
