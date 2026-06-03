#!/usr/bin/env bash
# Smoke-test the UniFFI Swift bindings for personal-rns-ffi on macOS.
#
# This intentionally does not commit generated Swift. It exercises the same UDL
# surface that Android/Kotlin uses by generating Swift into a temp dir, compiling
# a tiny native Swift caller, and running it against the local Rust dylib.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v swiftc >/dev/null 2>&1; then
  echo "swift-ffi-smoke: swiftc is required" >&2
  exit 1
fi

swift_target="$(
  swiftc -print-target-info \
    | sed -n 's/.*"triple"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
    | head -n 1
)"
if [ -z "${swift_target}" ]; then
  echo "swift-ffi-smoke: could not determine Swift target triple" >&2
  exit 1
fi

tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/prns-swift-smoke.XXXXXX")"
trap 'rm -rf "$tmpdir"' EXIT

echo "[rust] build personal-rns-ffi dylib"
cargo build -p personal-rns-ffi

echo "[uniffi] generate Swift bindings"
cargo run -p personal-rns-ffi --bin uniffi-bindgen -- generate \
  personal-rns-ffi/src/prns.udl \
  --language swift \
  --no-format \
  --out-dir "${tmpdir}"

smoke_swift="${tmpdir}/main.swift"
cat >"${smoke_swift}" <<'SWIFT'
let runtime = ReticulumRuntime()

let initialTickCount = runtime.tickCount()
let emitted = runtime.tick()
let finalTickCount = runtime.tickCount()
let linkedVersion = version()

guard initialTickCount == 0 else {
    fatalError("expected initial tick count 0, got \(initialTickCount)")
}
guard emitted == 0 else {
    fatalError("expected no emitted packets, got \(emitted)")
}
guard finalTickCount == 1 else {
    fatalError("expected final tick count 1, got \(finalTickCount)")
}
guard !linkedVersion.isEmpty else {
    fatalError("expected non-empty personal-rns-ffi version")
}

print("prns swift smoke ok: version=\(linkedVersion) ticks=\(finalTickCount)")
SWIFT

echo "[swift] compile smoke executable"
swiftc \
  -target "${swift_target}" \
  -module-cache-path "${tmpdir}/ModuleCache" \
  -I "${tmpdir}" \
  -Xcc "-fmodule-map-file=${tmpdir}/prnsFFI.modulemap" \
  "${tmpdir}/prns.swift" \
  "${smoke_swift}" \
  -L target/debug \
  -lpersonal_rns_ffi \
  -Xlinker -rpath \
  -Xlinker target/debug \
  -o "${tmpdir}/prns-swift-smoke"

echo "[swift] run smoke executable"
"${tmpdir}/prns-swift-smoke"

echo "SWIFT_FFI_SMOKE_OK"
