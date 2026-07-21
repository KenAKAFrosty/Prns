#!/usr/bin/env bash
# Forcing-function gate: the Personal Reticulum core must cross-compile to
# Android. Builds the core (rlib) for the primary device + emulator ABIs. The
# JNI cdylib + NDK link land with the bindings chunk; this gate is the
# core-compiles axis only.
set -euo pipefail
cd "$(dirname "$0")/.."

for target in aarch64-linux-android x86_64-linux-android; do
  echo "[android] core -> ${target}"
  cargo build --locked -p personal-rns --target "${target}"
done

echo "ANDROID_BUILD_GATE_OK"
