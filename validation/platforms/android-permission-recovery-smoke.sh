#!/usr/bin/env bash
set -euo pipefail

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

grant_all() {
  for permission in \
    android.permission.POST_NOTIFICATIONS \
    android.permission.NEARBY_WIFI_DEVICES \
    android.permission.BLUETOOTH_SCAN \
    android.permission.BLUETOOTH_ADVERTISE \
    android.permission.BLUETOOTH_CONNECT
  do
    adb_cmd shell pm grant org.personal.hopspot "${permission}" >/dev/null 2>&1 || true
  done
}

result_value() {
  local output="$1"
  local name="$2"
  printf '%s\n' "${output}" |
    sed -n "s/^INSTRUMENTATION_RESULT: ${name}=//p" |
    tr -d '\r'
}

run_probe() {
  set +e
  probe_output="$(adb_cmd shell am instrument -w \
    org.personal.hopspot.test/org.personal.hopspot.PrnsRuntimeProbe 2>&1)"
  probe_status=$?
  set -e
  printf '%s\n' "${probe_output}"
  if [[ "${probe_status}" -ne 0 ]] ||
    [[ "${probe_output}" != *"INSTRUMENTATION_RESULT: status=ok"* ]]
  then
    echo "PrnsRuntimeProbe failed" >&2
    exit 1
  fi
}

assert_result() {
  local name="$1"
  local expected="$2"
  local found
  found="$(result_value "${probe_output}" "${name}")"
  if [[ "${found}" != "${expected}" ]]; then
    echo "${name}: expected ${expected}, found ${found}" >&2
    exit 1
  fi
}

if [[ "$(adb_cmd get-state 2>/dev/null | tr -d '\r')" != "device" ]]; then
  echo "the selected Android device is not online" >&2
  exit 2
fi
api="$(adb_cmd shell getprop ro.build.version.sdk | tr -d '\r')"
if [[ ! "${api}" =~ ^[0-9]+$ ]] || (( api < 33 )); then
  echo "permission recovery smoke requires Android API 33 or newer" >&2
  exit 2
fi
trap grant_all EXIT HUP INT TERM

adb_cmd shell am force-stop org.personal.hopspot
for permission in \
  android.permission.POST_NOTIFICATIONS \
  android.permission.NEARBY_WIFI_DEVICES \
  android.permission.BLUETOOTH_SCAN \
  android.permission.BLUETOOTH_ADVERTISE \
  android.permission.BLUETOOTH_CONNECT
do
  adb_cmd shell pm revoke org.personal.hopspot "${permission}"
done

echo "[android-permissions] full denial"
run_probe
assert_result ble_link_started false
assert_result wifi_aware_link_started true
assert_result wifi_aware_failure "Wi-Fi Aware needs the nearby-devices permission"
assert_result wifi_direct_link_started false
assert_result wifi_direct_failure "experimental Wi-Fi P2P is disabled in this build"

echo "[android-permissions] nearby Wi-Fi only"
adb_cmd shell pm grant org.personal.hopspot android.permission.NEARBY_WIFI_DEVICES
run_probe
assert_result ble_link_started false
assert_result wifi_aware_link_started true
assert_result wifi_aware_failure none

echo "[android-permissions] later Bluetooth grant"
for permission in \
  android.permission.BLUETOOTH_SCAN \
  android.permission.BLUETOOTH_ADVERTISE \
  android.permission.BLUETOOTH_CONNECT
do
  adb_cmd shell pm grant org.personal.hopspot "${permission}"
done
run_probe
assert_result ble_link_started true
assert_result wifi_aware_link_started true
assert_result wifi_aware_failure none

grant_all
trap - EXIT HUP INT TERM
echo "ANDROID_PERMISSION_RECOVERY_SMOKE_OK"
