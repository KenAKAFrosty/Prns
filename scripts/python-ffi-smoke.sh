#!/usr/bin/env bash
# Smoke-test the UniFFI Python bindings for personal-rns-ffi.
#
# This intentionally does not commit generated Python. It generates the bindings
# into a temp dir, places the local Rust cdylib where UniFFI's Python loader
# expects it, imports the module, and exercises the same version/tick surface as
# the Swift smoke.
set -euo pipefail
cd "$(dirname "$0")/.."

python_bin="${PYTHON:-python3}"
if ! command -v "${python_bin}" >/dev/null 2>&1; then
  echo "python-ffi-smoke: python3 is required" >&2
  exit 1
fi

case "$(uname -s)" in
  Darwin)
    built_lib="target/debug/libpersonal_rns_ffi.dylib"
    uniffi_lib="libuniffi_prns.dylib"
    ;;
  Linux)
    built_lib="target/debug/libpersonal_rns_ffi.so"
    uniffi_lib="libuniffi_prns.so"
    ;;
  *)
    echo "python-ffi-smoke: unsupported host $(uname -s)" >&2
    exit 1
    ;;
esac

tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/prns-python-smoke.XXXXXX")"
trap 'rm -rf "$tmpdir"' EXIT

echo "[rust] build personal-rns-ffi dylib"
cargo build -p personal-rns-ffi

echo "[uniffi] generate Python bindings"
cargo run -p personal-rns-ffi --bin uniffi-bindgen -- generate \
  personal-rns-ffi/src/prns.udl \
  --language python \
  --no-format \
  --out-dir "${tmpdir}"

cp "${built_lib}" "${tmpdir}/${uniffi_lib}"

echo "[python] run smoke import"
PYTHONPATH="${tmpdir}" "${python_bin}" - <<'PYTHON'
import prns

runtime = prns.ReticulumRuntime()
initial_tick_count = runtime.tick_count()
emitted = runtime.tick()
final_tick_count = runtime.tick_count()
linked_version = prns.version()

assert initial_tick_count == 0, f"expected initial tick count 0, got {initial_tick_count}"
assert emitted == 0, f"expected no emitted packets, got {emitted}"
assert final_tick_count == 1, f"expected final tick count 1, got {final_tick_count}"
assert linked_version, "expected non-empty personal-rns-ffi version"

print(f"prns python smoke ok: version={linked_version} ticks={final_tick_count}")
PYTHON

echo "PYTHON_FFI_SMOKE_OK"
