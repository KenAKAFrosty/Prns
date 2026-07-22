#!/usr/bin/env bash
# Install the Hopspot debug APK plus its same-signature instrumentation probe,
# then prove the foreground PrnsService stays alive while backgrounded and can
# be queried through the public shared-instance Messenger contract.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
android_dir="${repo_root}/personal-hopspot/mobile/android"
apk="${android_dir}/app/build/outputs/apk/debug/app-debug.apk"
test_apk="${android_dir}/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-${TMPDIR:-/tmp}/prns-gradle-home}"

adb_bin="${ADB:-adb}"
serial_args=()
if [[ -n "${ANDROID_SERIAL:-}" ]]; then
  serial_args=(-s "${ANDROID_SERIAL}")
fi

adb_cmd() {
  if [[ "${#serial_args[@]}" -gt 0 ]]; then
    "${adb_bin}" "${serial_args[@]}" "$@"
  else
    "${adb_bin}" "$@"
  fi
}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 127
  fi
}

need "${adb_bin}"

install_or_explain_signature_mismatch() {
  local package="$1"
  local apk_path="$2"
  set +e
  local output
  output="$(adb_cmd install -r "${apk_path}" 2>&1)"
  local status=$?
  set -e
  if [[ "${status}" -eq 0 ]]; then
    return 0
  fi
  if [[ "${output}" == *"INSTALL_FAILED_UPDATE_INCOMPATIBLE"* ]]; then
    if [[ "${PRNS_ANDROID_REINSTALL:-0}" == "1" ]]; then
      echo "[android-runtime] uninstalling ${package} because installed signature differs"
      adb_cmd uninstall "${package}" >/dev/null 2>&1 || true
      adb_cmd install -r "${apk_path}" >/dev/null
      return 0
    fi
    printf '%s\n' "${output}" >&2
    echo "installed ${package} has a different signature; rerun with PRNS_ANDROID_REINSTALL=1 to uninstall/reinstall the debug APK" >&2
    exit 3
  fi
  printf '%s\n' "${output}" >&2
  exit "${status}"
}

if [[ "${PRNS_ANDROID_SKIP_BUILD:-0}" != "1" ]]; then
  bash "${repo_root}/validation/platforms/android-service-smoke.sh"
fi

echo "[android-runtime] assemble instrumentation probe"
(
  cd "${android_dir}"
  ./gradlew --no-daemon :app:assembleDebugAndroidTest
)

if [[ ! -f "${apk}" ]]; then
  echo "missing debug APK at ${apk}" >&2
  exit 1
fi
if [[ ! -f "${test_apk}" ]]; then
  echo "missing instrumentation APK at ${test_apk}" >&2
  exit 1
fi

if [[ -n "${ANDROID_SERIAL:-}" ]]; then
  if [[ "$(adb_cmd get-state 2>/dev/null | tr -d '\r')" != "device" ]]; then
    echo "ANDROID_SERIAL=${ANDROID_SERIAL} is not online" >&2
    exit 2
  fi
else
  devices="$("${adb_bin}" devices | sed -n '2,$p' | awk '$2 == "device" { print $1 }')"
  if [[ -z "${devices}" ]]; then
    echo "no online Android device; set ANDROID_SERIAL when multiple devices are attached" >&2
    exit 2
  fi
  if [[ "$(printf '%s\n' "${devices}" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "multiple online Android devices; set ANDROID_SERIAL" >&2
    printf '%s\n' "${devices}" >&2
    exit 2
  fi
fi

echo "[android-runtime] install app and probe"
install_or_explain_signature_mismatch org.personal.hopspot "${apk}"
install_or_explain_signature_mismatch org.personal.hopspot.test "${test_apk}"

api="$(adb_cmd shell getprop ro.build.version.sdk | tr -d '\r')"
if [[ "${api}" =~ ^[0-9]+$ ]]; then
  if (( api >= 23 )); then
    adb_cmd shell pm grant org.personal.hopspot.test android.permission.POST_NOTIFICATIONS >/dev/null 2>&1 || true
    adb_cmd shell pm grant org.personal.hopspot android.permission.ACCESS_FINE_LOCATION >/dev/null 2>&1 || true
  fi
  if (( api >= 31 )); then
    adb_cmd shell pm grant org.personal.hopspot android.permission.BLUETOOTH_SCAN >/dev/null 2>&1 || true
    adb_cmd shell pm grant org.personal.hopspot android.permission.BLUETOOTH_CONNECT >/dev/null 2>&1 || true
    adb_cmd shell pm grant org.personal.hopspot android.permission.BLUETOOTH_ADVERTISE >/dev/null 2>&1 || true
  fi
  if (( api >= 33 )); then
    adb_cmd shell pm grant org.personal.hopspot android.permission.POST_NOTIFICATIONS >/dev/null 2>&1 || true
  fi
fi

echo "[android-runtime] run foreground-service runtime probe"
set +e
probe_output="$(adb_cmd shell am instrument -w org.personal.hopspot.test/org.personal.hopspot.PrnsRuntimeProbe 2>&1)"
probe_status=$?
set -e
printf '%s\n' "${probe_output}"
if [[ "${probe_status}" -ne 0 ]] || [[ "${probe_output}" != *"INSTRUMENTATION_RESULT: status=ok"* ]]; then
  echo "PrnsRuntimeProbe failed" >&2
  exit 1
fi

services="$(adb_cmd shell dumpsys activity services org.personal.hopspot 2>/dev/null || true)"
if [[ "${services}" != *"PrnsService"* ]]; then
  echo "note: PrnsService is no longer listed after instrumentation teardown; in-probe dump asserted it while active"
fi

notifications="$(adb_cmd shell dumpsys notification --noredact 2>/dev/null || true)"
if [[ -n "${notifications}" ]] && [[ "${notifications}" != *"Personal RNS"* ]] && [[ "${notifications}" != *"personal_rns_node"* ]]; then
  echo "warning: notification dump did not expose the Personal RNS notification; service bind proof passed" >&2
fi

adb_cmd shell am startservice \
  -n org.personal.hopspot/.PrnsService \
  -a org.personal.hopspot.action.STOP_PRNS >/dev/null 2>&1 || true

echo "ANDROID_RUNTIME_SMOKE_OK"
