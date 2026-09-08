#!/bin/bash
set -euo pipefail

SCRIPT_DIRECTORY="$(cd "$(dirname "$0")" && pwd)"
APPLICATIONS_DIRECTORY="$(cd "${SCRIPT_DIRECTORY}/../../.." && pwd)"
APP_DIRECTORY="${APPLICATIONS_DIRECTORY}/prns/app"
case "${1:-}" in
  "") BUILD_VARIANT=debug; GRADLE_TASK=assembleDebug ;;
  --standalone) BUILD_VARIANT=release; GRADLE_TASK=assembleRelease ;;
  *) echo 'usage: build-client.sh [--standalone]' >&2; exit 1 ;;
esac
[[ $# -le 1 ]] || { echo 'Unexpected build arguments.' >&2; exit 1; }
[[ -n "${ANDROID_HOME:-}" ]] || { echo 'Set ANDROID_HOME to your Android SDK directory.' >&2; exit 1; }
[[ -x "${APPLICATIONS_DIRECTORY}/node_modules/.bin/expo" ]] || { echo 'Run npm ci in applications first.' >&2; exit 1; }

# Regenerate only Android, preserving the independently generated iOS project.
(
  cd "${APP_DIRECTORY}"
  env CI=1 PRNS_APP_VARIANT=development ../../node_modules/.bin/expo prebuild --platform android --no-install
)
(
  cd "${APP_DIRECTORY}/android"
  ./gradlew ":app:${GRADLE_TASK}" :prns-internal-expo:testDebugUnitTest \
    --max-workers="${PRNS_ANDROID_BUILD_WORKERS:-4}" --console=plain \
    -PreactNativeArchitectures="${PRNS_ANDROID_ABIS:-arm64-v8a}"
)
APK="${APP_DIRECTORY}/android/app/build/outputs/apk/${BUILD_VARIANT}/app-${BUILD_VARIANT}.apk"
[[ -f "${APK}" ]] || { echo 'The Android build did not produce an APK.' >&2; exit 1; }
AAPT="${ANDROID_HOME}/build-tools/36.0.0/aapt2"
[[ -x "${AAPT}" ]] || { echo 'Android build tools 36.0.0 are required for APK verification.' >&2; exit 1; }
BADGING="$("${AAPT}" dump badging "${APK}")"
grep -Fq "package: name='rs.reticulum.prns.dev'" <<<"${BADGING}"
grep -Fxq "minSdkVersion:'29'" <<<"${BADGING}"
APK_ENTRIES="$(unzip -Z1 "${APK}")"
grep -Eq '^lib/(arm64-v8a|x86_64)/libprns_app\.so$' <<<"${APK_ENTRIES}"
if [[ "${BUILD_VARIANT}" == release ]]; then
  grep -Fx 'assets/index.android.bundle' <<<"${APK_ENTRIES}" >/dev/null
fi
"${ANDROID_HOME}/build-tools/36.0.0/zipalign" -c -P 16 4 "${APK}" >/dev/null
echo "Android ${BUILD_VARIANT} APK verified: ${APK}"
if [[ "${BUILD_VARIANT}" == release ]]; then
  echo 'Standalone development build: bundled JavaScript, development identifier, local debug signing. Not a production release.'
fi
