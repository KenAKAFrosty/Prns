#!/usr/bin/env bash
# Smoke-test the UniFFI Ruby bindings for personal-rns-ffi.
#
# This intentionally does not commit generated Ruby. It generates the bindings
# into a temp dir, places the local Rust cdylib where the Ruby FFI loader can
# find it, requires the generated module, and exercises the version/tick surface.
set -euo pipefail
cd "$(dirname "$0")/.."

ruby_bin="${RUBY:-ruby}"
if ! command -v "${ruby_bin}" >/dev/null 2>&1; then
  echo "ruby-ffi-smoke: ruby is required" >&2
  exit 1
fi

if ! "${ruby_bin}" -e 'begin; require "ffi"; rescue LoadError; exit 42; end' >/dev/null 2>&1; then
  cat >&2 <<'EOF'
ruby-ffi-smoke: the Ruby ffi gem is required.

Install it with your preferred Ruby manager, or try:
  gem install --user-install ffi
EOF
  exit 1
fi

case "$(uname -s)" in
  Darwin)
    built_lib="target/debug/libpersonal_rns_ffi.dylib"
    uniffi_lib="libuniffi_prns.dylib"
    lib_path_var="DYLD_LIBRARY_PATH"
    ;;
  Linux)
    built_lib="target/debug/libpersonal_rns_ffi.so"
    uniffi_lib="libuniffi_prns.so"
    lib_path_var="LD_LIBRARY_PATH"
    ;;
  *)
    echo "ruby-ffi-smoke: unsupported host $(uname -s)" >&2
    exit 1
    ;;
esac

tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/prns-ruby-smoke.XXXXXX")"
trap 'rm -rf "$tmpdir"' EXIT

echo "[rust] build personal-rns-ffi dylib"
cargo build -p personal-rns-ffi

echo "[uniffi] generate Ruby bindings"
cargo run -p personal-rns-ffi --bin uniffi-bindgen -- generate \
  personal-rns-ffi/src/prns.udl \
  --language ruby \
  --no-format \
  --out-dir "${tmpdir}"

cp "${built_lib}" "${tmpdir}/${uniffi_lib}"

echo "[ruby] run smoke require"
env \
  RUBYLIB="${tmpdir}${RUBYLIB:+:${RUBYLIB}}" \
  "${lib_path_var}=${tmpdir}${!lib_path_var:+:${!lib_path_var}}" \
  "${ruby_bin}" - <<'RUBY'
require "prns"

runtime = Prns::ReticulumRuntime.new
initial_tick_count = runtime.tick_count
emitted = runtime.tick
final_tick_count = runtime.tick_count
linked_version = Prns.version

raise "expected initial tick count 0, got #{initial_tick_count}" unless initial_tick_count == 0
raise "expected no emitted packets, got #{emitted}" unless emitted == 0
raise "expected final tick count 1, got #{final_tick_count}" unless final_tick_count == 1
raise "expected non-empty personal-rns-ffi version" if linked_version.empty?

puts "prns ruby smoke ok: version=#{linked_version} ticks=#{final_tick_count}"
RUBY

echo "RUBY_FFI_SMOKE_OK"
