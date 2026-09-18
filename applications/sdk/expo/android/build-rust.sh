#!/bin/bash
set -euo pipefail

SCRIPT_DIRECTORY="$(cd "$(dirname "$0")" && pwd)"
APPLICATIONS_DIRECTORY="$(cd "${SCRIPT_DIRECTORY}/../../.." && pwd)"
# The generated NativeBindings package is the sole native-library owner.
# This command regenerates typed sources before building the shared image.
# Run before prebuild/autolinking; Expo must never package a second image.
python3 "${APPLICATIONS_DIRECTORY}/tools/generated-bindings/generate.py" android \
  --targets "${PRNS_ANDROID_ABIS:-arm64-v8a}" --release
