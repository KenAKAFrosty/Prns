#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

artifact_root="${PRNS_VALIDATION_ARTIFACTS:-$root/validation-artifacts}"
if [[ "$artifact_root" != /* ]]; then
  artifact_root="$root/$artifact_root"
fi
artifact_dir="$artifact_root/results/${PRNS_VALIDATION_SUITE:-ios-simulator}"
mkdir -p "$artifact_dir"

case "$(uname -m)" in
  arm64) rust_target="aarch64-apple-ios-sim" ;;
  x86_64) rust_target="x86_64-apple-ios" ;;
  *)
    echo "unsupported macOS host architecture: $(uname -m)" >&2
    exit 1
    ;;
esac
rustup target add "$rust_target"

selection="$(
  xcrun simctl list devices available -j |
    python3 -c '
import json, sys
payload = json.load(sys.stdin)
for runtime, devices in payload["devices"].items():
    for device in devices:
        if device.get("isAvailable") and device["name"].startswith(("iPhone", "iPad")):
            print("\t".join((device["udid"], device["name"], runtime, device["dataPath"])))
            raise SystemExit
raise SystemExit("no available iOS simulator")
'
)"
IFS=$'\t' read -r simulator_id simulator_name simulator_runtime simulator_data_path <<<"$selection"
test -n "$simulator_id"

derived_data="$(mktemp -d "${TMPDIR:-/tmp}/prns-ios-simulator-derived.XXXXXX")"
trap 'echo "ios-simulator: command failed at line $LINENO" >&2' ERR
trap 'rm -rf "$derived_data"' EXIT
app="$derived_data/Build/Products/Debug-iphonesimulator/PersonalHopspot.app"
screenshot="$derived_data/screenshot.png"
source_icon="personal-hopspot/mobile/ios/app/PersonalHopspot/Assets.xcassets/AppIcon.appiconset/AppIcon.png"
crash_root="$simulator_data_path/data/Library/Logs/CrashReporter"

record_crashes() {
  local output="$1"
  if [[ -d "$crash_root" ]]; then
    find "$crash_root" -type f -name 'PersonalHopspot*' -print | LC_ALL=C sort >"$output"
  else
    : >"$output"
  fi
}

record_crashes "$artifact_dir/crashes-before.txt"
xcrun simctl boot "$simulator_id" 2>/dev/null || true
xcrun simctl bootstatus "$simulator_id" -b

xcodebuild \
  -project personal-hopspot/mobile/ios/app/PersonalHopspot.xcodeproj \
  -scheme PersonalHopspot \
  -configuration Debug \
  -destination "id=$simulator_id" \
  -derivedDataPath "$derived_data" \
  build 2>&1 | tee "$artifact_dir/build.log"

test -f "$source_icon"
test "$(shasum -a 256 docs/website/public/assets/favicon.svg | awk '{print $1}')" = \
  "b91eb0b09ec1f4469e3c070033ce51c198332c5424a702fb6ad70de0424005a8"
test "$(shasum -a 256 "$source_icon" | awk '{print $1}')" = \
  "5d9c226fbe4f97c45913c7d84bdb770f8d82d1f5b3aaa95f72cb19f229ac1513"
test "$(sips -g pixelWidth "$source_icon" | awk '/pixelWidth/ {print $2}')" = "1024"
test "$(sips -g pixelHeight "$source_icon" | awk '/pixelHeight/ {print $2}')" = "1024"
test "$(sips -g hasAlpha "$source_icon" | awk '/hasAlpha/ {print $2}')" = "no"
test -f "$app/Assets.car"
test -s "$app/THIRD_PARTY_NOTICES.md"
code_binary="$app/PersonalHopspot"
if [[ -f "$app/PersonalHopspot.debug.dylib" ]]; then
  code_binary="$app/PersonalHopspot.debug.dylib"
fi
test -s "$code_binary"

compiled_info="$app/Info.plist"
test "$(plutil -extract CFBundleShortVersionString raw "$compiled_info")" = "0.1.0"
test "$(plutil -extract CFBundleVersion raw "$compiled_info")" = "1"
test -n "$(plutil -extract NSBluetoothAlwaysUsageDescription raw "$compiled_info")"
test -n "$(plutil -extract NSLocalNetworkUsageDescription raw "$compiled_info")"
plutil -extract NSBonjourServices json -o - "$compiled_info" | grep -qF '_reticulum._tcp'
plutil -extract UIBackgroundModes json -o - "$compiled_info" | grep -qF 'bluetooth-central'
plutil -extract UIBackgroundModes json -o - "$compiled_info" | grep -qF 'bluetooth-peripheral'

# Simulator processes share the host network namespace. Stop an earlier validation
# instance on every booted simulator so its production listener cannot mask this run.
while IFS= read -r booted_id; do
  xcrun simctl terminate "$booted_id" com.personal.hopspot 2>/dev/null || true
done < <(
  xcrun simctl list devices -j |
    python3 -c '
import json, sys
payload = json.load(sys.stdin)
for devices in payload["devices"].values():
    for device in devices:
        if device.get("state") == "Booted":
            print(device["udid"])
'
)

xcrun simctl install "$simulator_id" "$app"
launch_output="$(
  xcrun simctl launch --terminate-running-process \
    "$simulator_id" com.personal.hopspot
)"
printf '%s\n' "$launch_output" | tee "$artifact_dir/launch.txt"
launch_pid="$(printf '%s\n' "$launch_output" | awk -F': ' '/com\.personal\.hopspot:/ {print $2}')"
if ! [[ "$launch_pid" =~ ^[0-9]+$ ]]; then
  echo "simctl launch did not return a numeric PID: $launch_output" >&2
  exit 1
fi

lifecycle_deadline_seconds=30
lifecycle_poll_seconds=2
lifecycle_log_window=1m
lifecycle_start_marker='HOPSPOT_IOS_START result=0'
lifecycle_running_marker='HOPSPOT_IOS_STATE state=2 failure=0'
lifecycle_process_marker="PersonalHopspot[$launch_pid:"
lifecycle_deadline=$((SECONDS + lifecycle_deadline_seconds))
lifecycle_ready=false
lifecycle_failure=
launch_pid_observed=false
launchctl_pid=

lifecycle_log_has() {
  local marker="$1"
  grep -F "$lifecycle_process_marker" "$artifact_dir/lifecycle.log" |
    grep -qF "$marker"
}

while ((SECONDS < lifecycle_deadline)); do
  xcrun simctl spawn "$simulator_id" launchctl print user/501 \
    >"$artifact_dir/launchctl.txt" || true
  launchctl_line="$(
    grep -F 'UIKitApplication:com.personal.hopspot' \
      "$artifact_dir/launchctl.txt" || true
  )"
  launchctl_pid="$(printf '%s\n' "$launchctl_line" | awk 'NR == 1 {print $1}')"

  xcrun simctl spawn "$simulator_id" log show \
    --style compact \
    --last "$lifecycle_log_window" \
    --predicate 'subsystem == "com.personal.hopspot"' \
    >"$artifact_dir/lifecycle.log" \
    2>"$artifact_dir/lifecycle-stderr.log" || true

  if [[ -n "$launchctl_pid" && "$launchctl_pid" != "$launch_pid" ]]; then
    lifecycle_failure="launchctl reported PID $launchctl_pid instead of $launch_pid"
    break
  fi
  if [[ "$launch_pid_observed" == true && -z "$launchctl_pid" ]]; then
    lifecycle_failure="launchctl lost launched PID $launch_pid"
    break
  fi
  if [[ "$launchctl_pid" == "$launch_pid" ]]; then
    launch_pid_observed=true
  fi

  if ((SECONDS < lifecycle_deadline)) &&
    [[ "$launch_pid_observed" == true ]] &&
    lifecycle_log_has "$lifecycle_start_marker" &&
    lifecycle_log_has "$lifecycle_running_marker"; then
    lifecycle_ready=true
    break
  fi

  sleep "$lifecycle_poll_seconds"
done

xcrun simctl io "$simulator_id" screenshot "$screenshot"
test -s "$screenshot"
cp "$screenshot" "$artifact_dir/screenshot.png"
record_crashes "$artifact_dir/crashes-after.txt"
if comm -13 "$artifact_dir/crashes-before.txt" "$artifact_dir/crashes-after.txt" |
  tee "$artifact_dir/new-crashes.txt" |
  grep -q .; then
  echo "PersonalHopspot produced a new simulator crash report" >&2
  exit 1
fi
if [[ -n "$lifecycle_failure" ]]; then
  echo "$lifecycle_failure" >&2
  exit 1
fi
if [[ "$lifecycle_ready" != true ]]; then
  echo "iOS lifecycle proof did not settle within ${lifecycle_deadline_seconds}s" >&2
  exit 1
fi
test -s "$artifact_dir/screenshot.png"

{
  echo "simulator_id=$simulator_id"
  echo "simulator_name=$simulator_name"
  echo "simulator_runtime=$simulator_runtime"
  echo "host_arch=$(uname -m)"
  echo "rust_target=$rust_target"
  echo "launch_pid=$launch_pid"
  echo "version=$(plutil -extract CFBundleShortVersionString raw "$compiled_info")"
  echo "build=$(plutil -extract CFBundleVersion raw "$compiled_info")"
  echo "icon_sha256=$(shasum -a 256 "$source_icon" | awk '{print $1}')"
  echo "binary_file=$(basename "$code_binary")"
  echo "binary_sha256=$(shasum -a 256 "$code_binary" | awk '{print $1}')"
} >"$artifact_dir/metadata.txt"

echo "IOS_SIMULATOR_GATE_OK id=$simulator_id name=$simulator_name pid=$launch_pid"
