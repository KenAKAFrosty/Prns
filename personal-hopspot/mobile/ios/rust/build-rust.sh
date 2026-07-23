#!/bin/bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"

MANIFEST_DIR="$(cd "$(dirname "$0")" && pwd)"

case "${PLATFORM_NAME:-}" in
  iphonesimulator)
    case "${ARCHS:-arm64}" in
      *x86_64*) TRIPLE="x86_64-apple-ios" ;;
      *) TRIPLE="aarch64-apple-ios-sim" ;;
    esac
    ;;
  iphoneos)
    TRIPLE="aarch64-apple-ios"
    ;;
  *)
    echo "build-rust.sh: set PLATFORM_NAME (iphonesimulator|iphoneos); got '${PLATFORM_NAME:-unset}'" >&2
    exit 1
    ;;
esac

rustup target add "${TRIPLE}" >/dev/null 2>&1 || true
cargo build --release --locked --target "${TRIPLE}" --manifest-path "${MANIFEST_DIR}/Cargo.toml"
