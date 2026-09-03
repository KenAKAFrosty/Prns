#!/bin/bash
set -euo pipefail

SCRIPT_DIRECTORY="$(cd "$(dirname "$0")" && pwd)"
APPLICATIONS_DIRECTORY="$(cd "${SCRIPT_DIRECTORY}/../../.." && pwd)"
APP_DIRECTORY="${APPLICATIONS_DIRECTORY}/prns/app"
IOS_DIRECTORY="${APP_DIRECTORY}/ios"
EXPO_EXECUTABLE="${APPLICATIONS_DIRECTORY}/node_modules/.bin/expo"
INFO_PLIST="${IOS_DIRECTORY}/prnsdev/Info.plist"
WORKSPACE="${IOS_DIRECTORY}/prnsdev.xcworkspace"
SCHEME="prnsdev"
DERIVED_DATA="${PRNS_IOS_DERIVED_DATA:-${APPLICATIONS_DIRECTORY}/target/ios-development-client}"
BLUETOOTH_USAGE="prns uses Bluetooth to connect to nearby Reticulum nodes."
LOCAL_NETWORK_USAGE="prns uses the local network for an explicitly configured development LXMF peer."

fail() {
  echo "build-development-client.sh: $*" >&2
  exit 1
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-development-client.sh: Apple toolchain unavailable; skipping the iOS development client"
  exit 0
fi

for executable in xcodebuild xcrun plutil; do
  command -v "${executable}" >/dev/null || fail "${executable} is required"
done
[[ -x "${EXPO_EXECUTABLE}" ]] || fail "run npm ci in ${APPLICATIONS_DIRECTORY} first"

POD_EXECUTABLE="${PRNS_POD_EXECUTABLE:-$(command -v pod || true)}"
[[ -n "${POD_EXECUTABLE}" ]] || fail "CocoaPods is required"

echo "build-development-client.sh: generating a clean development iOS project"
(
  cd "${APP_DIRECTORY}"
  env CI=1 PRNS_APP_VARIANT=development "${EXPO_EXECUTABLE}" prebuild \
    --clean \
    --platform ios \
    --no-install
)

[[ -f "${INFO_PLIST}" ]] || fail "clean CNG did not render ${INFO_PLIST}"
[[ "$(plutil -extract NSBluetoothAlwaysUsageDescription raw "${INFO_PLIST}")" == "${BLUETOOTH_USAGE}" ]] ||
  fail "clean CNG rendered unexpected Bluetooth usage copy"
[[ "$(plutil -extract NSLocalNetworkUsageDescription raw "${INFO_PLIST}")" == "${LOCAL_NETWORK_USAGE}" ]] ||
  fail "clean CNG rendered unexpected local-network usage copy"
if plutil -extract UIBackgroundModes raw "${INFO_PLIST}" >/dev/null 2>&1; then
  fail "clean CNG must not claim background execution"
fi

echo "build-development-client.sh: installing CocoaPods dependencies"
(
  cd "${IOS_DIRECTORY}"
  env -u LIBRARY_PATH "${POD_EXECUTABLE}" install
)
[[ -d "${WORKSPACE}" ]] || fail "CocoaPods did not generate ${WORKSPACE}"
grep -Fq "PrnsApp" "${IOS_DIRECTORY}/Podfile.lock" || fail "PrnsApp was not autolinked"

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
  build

APP_BUNDLE="${DERIVED_DATA}/Build/Products/Debug-iphonesimulator/prnsdev.app"
BUILT_INFO_PLIST="${APP_BUNDLE}/Info.plist"
[[ -d "${APP_BUNDLE}" ]] || fail "development client was not produced at ${APP_BUNDLE}"
[[ "$(plutil -extract CFBundleIdentifier raw "${BUILT_INFO_PLIST}")" == "rs.reticulum.prns.dev" ]] ||
  fail "development client has the wrong bundle identifier"

echo "build-development-client.sh: installing and launching on ${SIMULATOR_NAME} (${SIMULATOR_ID})"
xcrun simctl boot "${SIMULATOR_ID}" 2>/dev/null || true
xcrun simctl bootstatus "${SIMULATOR_ID}" -b
xcrun simctl terminate "${SIMULATOR_ID}" rs.reticulum.prns.dev 2>/dev/null || true
xcrun simctl install "${SIMULATOR_ID}" "${APP_BUNDLE}"
xcrun simctl launch "${SIMULATOR_ID}" rs.reticulum.prns.dev

echo "IOS_DEVELOPMENT_CLIENT_OK app=${APP_BUNDLE} simulator=${SIMULATOR_ID} name=${SIMULATOR_NAME}"
