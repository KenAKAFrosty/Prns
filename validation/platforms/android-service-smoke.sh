#!/usr/bin/env bash
# Build the Android JNI face and APK, then assert the foreground service and
# shared-instance bind contract are present in the merged manifest/package.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
android_dir="${repo_root}/personal-hopspot/mobile/android"
rust_dir="${android_dir}/rust"
apk="${android_dir}/app/build/outputs/apk/debug/app-debug.apk"
lab_apk="${android_dir}/app/build/outputs/apk/wifiDirectLab/app-wifiDirectLab.apk"
release_apk="${android_dir}/app/build/outputs/apk/release/app-release-unsigned.apk"
test_apk="${android_dir}/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-${TMPDIR:-/tmp}/prns-gradle-home}"
android_sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
aapt2="${android_sdk}/build-tools/34.0.0/aapt2"

if ! cargo ndk --version >/dev/null 2>&1; then
  echo "cargo-ndk is required; install with: cargo install cargo-ndk" >&2
  exit 127
fi
if [[ ! -x "${aapt2}" ]]; then
  echo "Android build-tools 34.0.0 aapt2 is required" >&2
  exit 127
fi

echo "[android] host Rust tests and lint"
cargo test --locked --manifest-path "${repo_root}/personal-hopspot/core/Cargo.toml"
cargo clippy --locked --manifest-path "${repo_root}/personal-hopspot/core/Cargo.toml" --all-targets -- -D warnings
cargo test --locked --manifest-path "${rust_dir}/Cargo.toml"
cargo clippy --locked --manifest-path "${rust_dir}/Cargo.toml" --all-targets -- -D warnings
cargo test --locked --manifest-path "${repo_root}/prns-ffi/Cargo.toml"
cargo clippy --locked --manifest-path "${repo_root}/prns-ffi/Cargo.toml" --all-targets -- -D warnings
cargo test --locked \
  --manifest-path "${repo_root}/prns-interfaces/impls/tokio/Cargo.toml" \
  --features wifi-direct \
  wifi_direct::runtime::tests::toggle_sleep_and_wake_change_the_aggregate_state

echo "[android] JNI -> arm64-v8a"
(
  cd "${rust_dir}"
  cargo ndk -t arm64-v8a -o ../app/src/main/jniLibs build --release --locked
)

echo "[android] JNI -> armeabi-v7a"
(
  cd "${rust_dir}"
  cargo ndk -t armeabi-v7a -P 21 -o ../app/src/main/jniLibs build --release --locked
)

echo "[android] dependency, lint, test, and APK gates"
(
  cd "${android_dir}"
  ./gradlew --no-daemon \
    :app:verifyExperimentalWifiDirectDisabled \
    :app:verifyReleaseRuntimeDependencies \
    :app:lint \
    :app:test \
    :app:assembleDebug \
    :app:assembleWifiDirectLab \
    :app:assembleDebugAndroidTest \
    :app:assembleRelease
)

if [[ ! -f "${apk}" ]]; then
  echo "missing debug APK at ${apk}" >&2
  exit 1
fi
if [[ ! -f "${release_apk}" ]]; then
  echo "missing unsigned release APK at ${release_apk}" >&2
  exit 1
fi
if [[ ! -f "${lab_apk}" ]]; then
  echo "missing isolated Wi-Fi Direct lab APK at ${lab_apk}" >&2
  exit 1
fi
if [[ ! -f "${test_apk}" ]]; then
  echo "missing foreground-service instrumentation APK at ${test_apk}" >&2
  exit 1
fi

manifest=""
for candidate in \
  "${android_dir}/app/build/intermediates/merged_manifests/debug/processDebugManifest/AndroidManifest.xml" \
  "${android_dir}/app/build/intermediates/merged_manifest/debug/processDebugMainManifest/AndroidManifest.xml" \
  "${android_dir}/app/build/intermediates/packaged_manifests/debug/processDebugManifestForPackage/AndroidManifest.xml"
do
  if [[ -f "${candidate}" ]]; then
    manifest="${candidate}"
    break
  fi
done

if [[ -z "${manifest}" ]]; then
  echo "could not find the merged AndroidManifest.xml" >&2
  exit 1
fi

lab_manifest=""
for candidate in \
  "${android_dir}/app/build/intermediates/merged_manifests/wifiDirectLab/processWifiDirectLabManifest/AndroidManifest.xml" \
  "${android_dir}/app/build/intermediates/merged_manifest/wifiDirectLab/processWifiDirectLabMainManifest/AndroidManifest.xml" \
  "${android_dir}/app/build/intermediates/packaged_manifests/wifiDirectLab/processWifiDirectLabManifestForPackage/AndroidManifest.xml"
do
  if [[ -f "${candidate}" ]]; then
    lab_manifest="${candidate}"
    break
  fi
done

if [[ -z "${lab_manifest}" ]]; then
  echo "could not find the merged Wi-Fi Direct lab AndroidManifest.xml" >&2
  exit 1
fi
lab_service="$(sed -n '/<service/,/<\/service>/p' "${lab_manifest}")"

grep -qF 'PrnsService' "${manifest}" || {
  echo "merged manifest is missing PrnsService" >&2
  exit 1
}
grep -qF 'org.personal.hopspot.permission.PRNS_CLIENT' "${manifest}" || {
  echo "merged manifest is missing the signature PRNS client permission" >&2
  exit 1
}
grep -qF 'org.personal.hopspot.action.BIND_PRNS_CLIENT' "${manifest}" || {
  echo "merged manifest is missing the shared-instance bind action" >&2
  exit 1
}
grep -qF 'connectedDevice' "${manifest}" || {
  echo "merged manifest is missing the connectedDevice foreground-service type" >&2
  exit 1
}
grep -qF 'android.permission.NEARBY_WIFI_DEVICES' "${manifest}" || {
  echo "merged manifest is missing the API 33 nearby Wi-Fi permission" >&2
  exit 1
}
grep -qF 'android.permission.ACCESS_FINE_LOCATION' "${manifest}" || {
  echo "merged manifest is missing the through-API-32 location permission" >&2
  exit 1
}
grep -qF 'android:maxSdkVersion="32"' "${manifest}" || {
  echo "merged manifest does not retain location through API 32" >&2
  exit 1
}
grep -qF 'android:allowBackup="false"' "${manifest}" || {
  echo "merged manifest permits application backup" >&2
  exit 1
}
grep -qF 'android:fullBackupContent="false"' "${manifest}" || {
  echo "merged manifest does not disable full backup" >&2
  exit 1
}
grep -qF 'android:dataExtractionRules="@xml/data_extraction_rules"' "${manifest}" || {
  echo "merged manifest is missing the Android 12 data-extraction exclusion" >&2
  exit 1
}
grep -qF 'android:icon="@mipmap/ic_launcher"' "${manifest}" || {
  echo "merged manifest is missing the Prns launcher icon" >&2
  exit 1
}
grep -qF 'android:versionCode="1"' "${manifest}" || {
  echo "merged manifest version code is not 1" >&2
  exit 1
}
grep -qF 'android:versionName="0.1.0"' "${manifest}" || {
  echo "merged manifest version name is not 0.1.0" >&2
  exit 1
}
grep -qF 'sourceCompatibility = JavaVersion.VERSION_1_8' "${android_dir}/app/build.gradle.kts" || {
  echo "Android source compatibility is not Java 8" >&2
  exit 1
}
grep -qF 'jvmTarget = "1.8"' "${android_dir}/app/build.gradle.kts" || {
  echo "Android Kotlin bytecode target is not Java 8" >&2
  exit 1
}
grep -qF 'package="org.personal.hopspot.wifidirectlab"' "${lab_manifest}" || {
  echo "Wi-Fi Direct lab does not use its isolated application ID" >&2
  exit 1
}
if grep -qF 'org.personal.hopspot.permission.PRNS_CLIENT' "${lab_manifest}"; then
  echo "Wi-Fi Direct lab retains the production signature permission" >&2
  exit 1
fi
[[ "${lab_service}" == *'android:exported="false"'* ]] || {
  echo "Wi-Fi Direct lab service is exported" >&2
  exit 1
}
[[ "${lab_service}" == *'android:foregroundServiceType="connectedDevice"'* ]] || {
  echo "Wi-Fi Direct lab service lost its foreground-service type" >&2
  exit 1
}
grep -qF 'android:label="Personal Hopspot Wi-Fi Direct Lab"' "${lab_manifest}" || {
  echo "Wi-Fi Direct lab is not visibly distinguished from the normal app" >&2
  exit 1
}
grep -qF 'public static final boolean EXPERIMENTAL_WIFI_DIRECT = true;' \
  "${android_dir}/app/build/generated/source/buildConfig/wifiDirectLab/org/personal/hopspot/BuildConfig.java" || {
  echo "Wi-Fi Direct lab does not enable the experimental transport" >&2
  exit 1
}

if command -v unzip >/dev/null 2>&1; then
  apk_listing="$(unzip -Z1 "${apk}")"
  release_listing="$(unzip -Z1 "${release_apk}")"
else
  apk_listing="$(jar tf "${apk}")"
  release_listing="$(jar tf "${release_apk}")"
fi

[[ "${apk_listing}" == *'lib/arm64-v8a/libpersonal_hopspot_android.so'* ]] || {
  echo "APK is missing the arm64-v8a JNI library" >&2
  exit 1
}
[[ "${apk_listing}" == *'lib/armeabi-v7a/libpersonal_hopspot_android.so'* ]] || {
  echo "APK is missing the armeabi-v7a JNI library" >&2
  exit 1
}
[[ "${apk_listing}" == *'assets/THIRD_PARTY_NOTICES.md'* ]] || {
  echo "APK is missing the checked third-party notice bundle" >&2
  exit 1
}
[[ "${release_listing}" == *'lib/arm64-v8a/libpersonal_hopspot_android.so'* ]] || {
  echo "release APK is missing the arm64-v8a JNI library" >&2
  exit 1
}
[[ "${release_listing}" == *'lib/armeabi-v7a/libpersonal_hopspot_android.so'* ]] || {
  echo "release APK is missing the armeabi-v7a JNI library" >&2
  exit 1
}
[[ "${release_listing}" == *'assets/THIRD_PARTY_NOTICES.md'* ]] || {
  echo "release APK is missing the checked third-party notice bundle" >&2
  exit 1
}
release_resources="$("${aapt2}" dump resources "${release_apk}")"
[[ "${release_resources}" == *'mipmap/ic_launcher'* ]] || {
  echo "release APK is missing the Prns launcher icon" >&2
  exit 1
}

echo "ANDROID_SERVICE_SMOKE_OK"
