#!/usr/bin/env bash
# Build the Android JNI face and APK, then assert the foreground service and
# shared-instance bind contract are present in the merged manifest/package.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
android_dir="${repo_root}/personal-hopspot/android"
rust_dir="${android_dir}/rust"
apk="${android_dir}/app/build/outputs/apk/debug/app-debug.apk"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-${TMPDIR:-/tmp}/prns-gradle-home}"

if ! cargo ndk --version >/dev/null 2>&1; then
  echo "cargo-ndk is required; install with: cargo install cargo-ndk" >&2
  exit 127
fi

echo "[android] JNI -> arm64-v8a"
(
  cd "${rust_dir}"
  cargo ndk -t arm64-v8a -o ../app/src/main/jniLibs build --release
)

echo "[android] JNI -> armeabi-v7a"
(
  cd "${rust_dir}"
  cargo ndk -t armeabi-v7a -P 21 -o ../app/src/main/jniLibs build --release
)

echo "[android] assemble debug APK"
(
  cd "${android_dir}"
  ./gradlew --no-daemon :app:assembleDebug
)

if [[ ! -f "${apk}" ]]; then
  echo "missing debug APK at ${apk}" >&2
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

grep -q 'PrnsService' "${manifest}" || {
  echo "merged manifest is missing PrnsService" >&2
  exit 1
}
grep -q 'org.personal.hopspot.permission.PRNS_CLIENT' "${manifest}" || {
  echo "merged manifest is missing the signature PRNS client permission" >&2
  exit 1
}
grep -q 'org.personal.hopspot.action.BIND_PRNS_CLIENT' "${manifest}" || {
  echo "merged manifest is missing the shared-instance bind action" >&2
  exit 1
}
grep -q 'connectedDevice' "${manifest}" || {
  echo "merged manifest is missing the connectedDevice foreground-service type" >&2
  exit 1
}

if command -v unzip >/dev/null 2>&1; then
  listing_cmd=(unzip -Z1 "${apk}")
else
  listing_cmd=(jar tf "${apk}")
fi

apk_listing="$("${listing_cmd[@]}")"

[[ "${apk_listing}" == *'lib/arm64-v8a/libpersonal_hopspot_android.so'* ]] || {
  echo "APK is missing the arm64-v8a JNI library" >&2
  exit 1
}
[[ "${apk_listing}" == *'lib/armeabi-v7a/libpersonal_hopspot_android.so'* ]] || {
  echo "APK is missing the armeabi-v7a JNI library" >&2
  exit 1
}

echo "ANDROID_SERVICE_SMOKE_OK"
