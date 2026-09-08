#!/bin/bash
set -euo pipefail

SCRIPT_DIRECTORY="$(cd "$(dirname "$0")" && pwd)"
APPLICATIONS_DIRECTORY="$(cd "${SCRIPT_DIRECTORY}/../../.." && pwd)"
NDK_DIRECTORY="${ANDROID_NDK_HOME:?ANDROID_NDK_HOME must point to an installed Android NDK}"
JNI_OUTPUT="${PRNS_ANDROID_JNI_OUTPUT:-${SCRIPT_DIRECTORY}/build/rustJniLibs}"
case "$(uname -s)" in
  Darwin) NDK_HOST=darwin-x86_64 ;;
  Linux) NDK_HOST=linux-x86_64 ;;
  *) echo 'Android Rust builds require macOS or Linux.' >&2; exit 1 ;;
esac
NDK_BIN="${NDK_DIRECTORY}/toolchains/llvm/prebuilt/${NDK_HOST}/bin"
[[ -x "${NDK_BIN}/llvm-ar" ]] || { echo 'Android NDK toolchain not found.' >&2; exit 1; }
export CARGO_TARGET_DIR="${PRNS_ANDROID_CARGO_TARGET_DIR:-${APPLICATIONS_DIRECTORY}/target/android}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
# Apply alignment to the Rust shared library as well as the APK container.
export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384"

IFS=',' read -r -a ABI_LIST <<< "${PRNS_ANDROID_ABIS:-arm64-v8a}"
for ABI in "${ABI_LIST[@]}"; do
  case "${ABI}" in
    arm64-v8a) RUST_TARGET=aarch64-linux-android; CLANG_TARGET=aarch64-linux-android ;;
    x86_64) RUST_TARGET=x86_64-linux-android; CLANG_TARGET=x86_64-linux-android ;;
    *) echo "Unsupported Android ABI: ${ABI}; supported: arm64-v8a,x86_64" >&2; exit 1 ;;
  esac
  rustup target list --installed | rg -Fx "${RUST_TARGET}" >/dev/null || {
    echo "Install the Rust target with: rustup target add ${RUST_TARGET}" >&2; exit 1;
  }
  LINKER="${NDK_BIN}/${CLANG_TARGET}29-clang"
  [[ -x "${LINKER}" ]] || { echo "Android API 29 linker missing: ${LINKER}" >&2; exit 1; }
  TARGET_VARIABLE="$(printf '%s' "${RUST_TARGET}" | tr '[:lower:]-' '[:upper:]_')"
  TARGET_UNDERSCORE="${RUST_TARGET//-/_}"
  env "CARGO_TARGET_${TARGET_VARIABLE}_LINKER=${LINKER}" \
    "CC_${TARGET_UNDERSCORE}=${LINKER}" \
    "AR_${TARGET_UNDERSCORE}=${NDK_BIN}/llvm-ar" \
    cargo build --release --locked --manifest-path "${APPLICATIONS_DIRECTORY}/Cargo.toml" \
      --package prns-app-native --features android --lib --target "${RUST_TARGET}"
  mkdir -p "${JNI_OUTPUT}/${ABI}"
  cp "${CARGO_TARGET_DIR}/${RUST_TARGET}/release/libprns_app.so" "${JNI_OUTPUT}/${ABI}/libprns_app.so"
done
