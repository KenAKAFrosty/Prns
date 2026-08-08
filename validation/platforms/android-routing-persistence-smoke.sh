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

result_value() {
  local output="$1"
  local name="$2"
  printf '%s\n' "${output}" |
    sed -n "s/.* ${name}=\\([^ ]*\\).*/\\1/p" |
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

probe_index=0
run_status_probe() {
  probe_index=$((probe_index + 1))
  local nonce="routing-$$-${probe_index}"
  adb_cmd shell am start -W \
    -n org.personal.hopspot.test/org.personal.hopspot.PrnsServiceHarnessActivity \
    --es nonce "${nonce}" >/dev/null
  probe_output=""
  for ((status_attempt = 1; status_attempt <= 25; status_attempt += 1)); do
    probe_output="$(adb_cmd logcat -d -v brief |
      sed -n "/STATUS nonce=${nonce} /{s/^.*STATUS /STATUS /;p;}" |
      tail -n 1)"
    if [[ -n "${probe_output}" ]]; then
      break
    fi
    sleep 0.2
  done
  probe_status=1
  if [[ "${probe_output}" == *"state=running"* ]] &&
    [[ "${probe_output}" == *"last_failure=none"* ]] &&
    [[ "${probe_output}" == *"persistence_active=true"* ]]
  then
    probe_status=0
  fi
}

if [[ "$(adb_cmd get-state 2>/dev/null | tr -d '\r')" != "device" ]]; then
  echo "the selected Android device is not online" >&2
  exit 2
fi
if ! adb_cmd shell pm path org.personal.hopspot >/dev/null 2>&1 ||
  ! adb_cmd shell pm path org.personal.hopspot.test >/dev/null 2>&1
then
  echo "install the current Hopspot app and instrumentation APKs first" >&2
  exit 2
fi

trap recover_target_on_failure EXIT
adb_cmd shell am start -W \
  -n org.personal.hopspot.test/org.personal.hopspot.PrnsServiceHarnessActivity \
  >/dev/null
sleep 2

echo "[android-routing-persistence] wait for a learned route"
route_observed=0
for ((attempt = 1; attempt <= 30; attempt += 1)); do
  run_status_probe
  route_count="$(result_value "${probe_output}" route_count)"
  if [[ "${probe_status}" -eq 0 ]] && [[ "${route_count}" =~ ^[1-9][0-9]*$ ]]; then
    route_observed=1
    break
  fi
  sleep 2
done
if [[ "${route_observed}" -ne 1 ]]; then
  printf '%s\n' "${probe_output}"
  echo "no learned route appeared within 60 seconds; keep a second RNS peer advertising" >&2
  exit 1
fi

baseline_flushes="$(result_value "${probe_output}" successful_flush_count)"
if [[ ! "${baseline_flushes}" =~ ^[0-9]+$ ]]; then
  printf '%s\n' "${probe_output}"
  echo "runtime persistence status was malformed" >&2
  exit 1
fi

echo "[android-routing-persistence] wait for the post-route periodic flush"
periodic_flush_observed=0
for ((attempt = 1; attempt <= 25; attempt += 1)); do
  sleep 2
  run_status_probe
  route_count="$(result_value "${probe_output}" route_count)"
  if [[ "${probe_status}" -ne 0 ]] || [[ ! "${route_count}" =~ ^[1-9][0-9]*$ ]]; then
    continue
  fi
  successful_flushes="$(result_value "${probe_output}" successful_flush_count)"
  if [[ "${successful_flushes}" =~ ^[0-9]+$ ]] &&
    (( successful_flushes > baseline_flushes ))
  then
    periodic_flush_observed=1
    break
  fi
done
if [[ "${periodic_flush_observed}" -ne 1 ]]; then
  printf '%s\n' "${probe_output}"
  echo "no periodic runtime-state flush landed within 50 seconds" >&2
  exit 1
fi

initial_pid="$(process_id)"
echo "[android-routing-persistence] inject process death after durable route flush"
adb_cmd shell run-as org.personal.hopspot kill -9 "${initial_pid}"

recreated=0
for ((attempt = 1; attempt <= 20; attempt += 1)); do
  sleep 1
  restarted_pid="$(process_id)"
  services="$(active_services || true)"
  if [[ -n "${restarted_pid}" ]] &&
    [[ "${restarted_pid}" != "${initial_pid}" ]] &&
    [[ "${services}" == *"PrnsService"* ]] &&
    [[ "${services}" == *"isForeground=true"* ]]
  then
    recreated=1
    break
  fi
done
if [[ "${recreated}" -ne 1 ]]; then
  echo "Android did not recreate the sticky foreground service within 20 seconds" >&2
  exit 1
fi

run_status_probe
printf '%s\n' "${probe_output}"
if [[ "${probe_status}" -ne 0 ]]; then
  echo "the crash-recreated engine did not restore its routing table" >&2
  exit 1
fi
restored="$(result_value "${probe_output}" restored_route_count)"
restored_destinations="$(result_value "${probe_output}" restored_destination_identity_count)"
restored_ratchets="$(result_value "${probe_output}" restored_ratchet_count)"
refused="$(result_value "${probe_output}" refused_restore_count)"
if [[ ! "${restored}" =~ ^[1-9][0-9]*$ ]] ||
  [[ ! "${restored_destinations}" =~ ^[1-9][0-9]*$ ]] ||
  [[ ! "${restored_ratchets}" =~ ^[1-9][0-9]*$ ]] ||
  [[ "${refused}" != "0" ]]
then
  echo "routing persistence evidence was incomplete after process recreation" >&2
  exit 1
fi

trap - EXIT
echo "ANDROID_ROUTING_PERSISTENCE_SMOKE_OK"
