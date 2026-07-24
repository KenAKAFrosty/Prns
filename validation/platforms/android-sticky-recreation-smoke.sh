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

recover_target_on_failure() {
  local status=$?
  trap - EXIT
  if [[ "${status}" -ne 0 ]]; then
    set +e
    adb_cmd shell am force-stop org.personal.hopspot
    sleep 1
    adb_cmd shell am start -W \
      -n org.personal.hopspot.test/org.personal.hopspot.PrnsServiceHarnessActivity \
      >/dev/null
  fi
  exit "${status}"
}

fingerprints=(
  rpc_identity_sha256
  node_identity_sha256
  bluetooth_identity_sha256
  delivery_destination_sha256
  node_page_destination_sha256
)

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
  for fingerprint in "${fingerprints[@]}"; do
    value="$(fingerprint_value "${probe_output}" "${fingerprint}")"
    if [[ ! "${value}" =~ ^[0-9a-f]{64}$ ]]; then
      echo "PrnsRuntimeProbe returned malformed ${fingerprint}" >&2
      exit 1
    fi
  done
}

fingerprint_value() {
  local output="$1"
  local name="$2"
  printf '%s\n' "${output}" |
    sed -n "s/^INSTRUMENTATION_RESULT: ${name}=//p" |
    tr -d '\r'
}

process_id() {
  local pid
  pid="$(adb_cmd shell pidof org.personal.hopspot 2>/dev/null | tr -d '\r' || true)"
  if [[ -n "${pid}" ]]; then
    printf '%s\n' "${pid}"
    return
  fi
  adb_cmd shell ps 2>/dev/null |
    awk '$NF == "org.personal.hopspot" { print $2; exit }'
}

active_services() {
  adb_cmd shell dumpsys activity services org.personal.hopspot 2>/dev/null |
    sed -n '/active services:/,$p'
}

if [[ "$(adb_cmd get-state 2>/dev/null | tr -d '\r')" != "device" ]]; then
  echo "the selected Android device is not online" >&2
  exit 2
fi
if ! adb_cmd shell pm path org.personal.hopspot >/dev/null 2>&1; then
  echo "org.personal.hopspot is not installed" >&2
  exit 2
fi
if ! adb_cmd shell pm path org.personal.hopspot.test >/dev/null 2>&1; then
  echo "org.personal.hopspot.test is not installed" >&2
  exit 2
fi

trap recover_target_on_failure EXIT
echo "[android-sticky] capture baseline identities"
run_probe
baseline_output="${probe_output}"

echo "[android-sticky] start protected service through same-signature harness"
adb_cmd shell am start -W \
  -n org.personal.hopspot.test/org.personal.hopspot.PrnsServiceHarnessActivity \
  >/dev/null
sleep 2

initial_pid="$(process_id)"
initial_services="$(active_services)"
if [[ -z "${initial_pid}" ]] ||
  [[ "${initial_services}" != *"PrnsService"* ]] ||
  [[ "${initial_services}" != *"isForeground=true"* ]]
then
  echo "same-signature harness did not start the foreground PrnsService" >&2
  exit 1
fi

echo "[android-sticky] inject package-process crash"
set +e
adb_cmd shell am crash org.personal.hopspot >/dev/null 2>&1
set -e

recreated=0
for ((attempt = 1; attempt <= 20; attempt += 1)); do
  sleep 1
  restarted_pid="$(process_id)"
  restarted_services="$(active_services || true)"
  if [[ -n "${restarted_pid}" ]] &&
    [[ "${restarted_pid}" != "${initial_pid}" ]] &&
    [[ "${restarted_services}" == *"PrnsService"* ]] &&
    [[ "${restarted_services}" == *"isForeground=true"* ]]
  then
    recreated=1
    break
  fi
done
if [[ "${recreated}" -ne 1 ]]; then
  echo "Android did not recreate the sticky foreground service within 20 seconds" >&2
  exit 1
fi

echo "[android-sticky] compare identities after recreation"
run_probe
recreated_output="${probe_output}"
for fingerprint in "${fingerprints[@]}"; do
  baseline="$(fingerprint_value "${baseline_output}" "${fingerprint}")"
  recreated_value="$(fingerprint_value "${recreated_output}" "${fingerprint}")"
  if [[ "${baseline}" != "${recreated_value}" ]]; then
    echo "${fingerprint} changed across sticky recreation" >&2
    exit 1
  fi
done

trap - EXIT
echo "ANDROID_STICKY_RECREATION_SMOKE_OK"
