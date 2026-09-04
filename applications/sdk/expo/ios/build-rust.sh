#!/bin/bash
set -euo pipefail

SCRIPT_DIRECTORY="$(cd "$(dirname "$0")" && pwd)"
APPLICATIONS_DIRECTORY="$(cd "${SCRIPT_DIRECTORY}/../../.." && pwd)"

case "${PLATFORM_NAME:-}" in
  iphonesimulator)
    case "${ARCHS:-arm64}" in
      *x86_64*) RUST_TARGET="x86_64-apple-ios" ;;
      *) RUST_TARGET="aarch64-apple-ios-sim" ;;
    esac
    ;;
  iphoneos)
    RUST_TARGET="aarch64-apple-ios"
    ;;
  *)
    echo "build-rust.sh: PLATFORM_NAME must be iphonesimulator or iphoneos; received '${PLATFORM_NAME:-unset}'" >&2
    exit 1
    ;;
esac

CARGO_EXECUTABLE="$(command -v cargo || true)"
RUSTUP_EXECUTABLE="$(command -v rustup || true)"
if [[ -z "${CARGO_EXECUTABLE}" && -x "${HOME}/.cargo/bin/cargo" ]]; then
  CARGO_EXECUTABLE="${HOME}/.cargo/bin/cargo"
fi
if [[ -z "${RUSTUP_EXECUTABLE}" && -x "${HOME}/.cargo/bin/rustup" ]]; then
  RUSTUP_EXECUTABLE="${HOME}/.cargo/bin/rustup"
fi
if [[ -z "${CARGO_EXECUTABLE}" || -z "${RUSTUP_EXECUTABLE}" ]]; then
  echo "build-rust.sh: cargo and rustup must be installed and discoverable" >&2
  exit 1
fi
if ! "${RUSTUP_EXECUTABLE}" target list --installed | grep -Fxq "${RUST_TARGET}"; then
  echo "build-rust.sh: missing Rust target ${RUST_TARGET}; run 'rustup target add ${RUST_TARGET}'" >&2
  exit 1
fi

export CARGO_TARGET_DIR="${SCRIPT_DIRECTORY}/build"
RUST_FEATURES="apple"
if [[ "${CONFIGURATION:-}" == "Debug" ]]; then
  RUST_FEATURES="${RUST_FEATURES},ios-restoration-probe"
fi
"${CARGO_EXECUTABLE}" build \
  --release \
  --locked \
  --manifest-path "${APPLICATIONS_DIRECTORY}/Cargo.toml" \
  --package prns-app-native \
  --features "${RUST_FEATURES}" \
  --lib \
  --target "${RUST_TARGET}"

ARCHIVE="${CARGO_TARGET_DIR}/${RUST_TARGET}/release/libprns_app.a"
if [[ -n "${PRNS_APP_ARCHIVE_OUTPUT:-}" ]]; then
  cp "${ARCHIVE}" "${PRNS_APP_ARCHIVE_OUTPUT}"
fi
