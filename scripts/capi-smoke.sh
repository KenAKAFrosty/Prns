#!/usr/bin/env bash
# Smoke-test the C ABI surface for personal-rns-capi.
#
# This compiles a tiny C consumer against the committed header and the local
# Rust cdylib so the native package shape stays honest as the API grows.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v cc >/dev/null 2>&1; then
  echo "capi-smoke: cc is required" >&2
  exit 1
fi

case "$(uname -s)" in
  Darwin)
    built_lib="target/debug/libpersonal_rns_capi.dylib"
    lib_env_name="DYLD_LIBRARY_PATH"
    ;;
  Linux)
    built_lib="target/debug/libpersonal_rns_capi.so"
    lib_env_name="LD_LIBRARY_PATH"
    ;;
  *)
    echo "capi-smoke: unsupported host $(uname -s)" >&2
    exit 1
    ;;
esac

tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/prns-capi-smoke.XXXXXX")"
trap 'rm -rf "$tmpdir"' EXIT

echo "[rust] build personal-rns-capi dylib"
cargo build -p personal-rns-capi

cat >"${tmpdir}/smoke.c" <<'C'
#include <stdint.h>
#include <stdio.h>

#include "personal_rns.h"

static int fail(const char *message, prns_status_t status) {
  fprintf(stderr, "%s: %s\n", message, prns_status_message(status));
  return 1;
}

int main(void) {
  if (prns_abi_version() != PRNS_ABI_VERSION) {
    return fail("unexpected ABI version", PRNS_STATUS_OK);
  }

  if (prns_version() == NULL || prns_version()[0] == '\0') {
    return fail("empty version", PRNS_STATUS_OK);
  }

  prns_runtime_t *runtime = NULL;
  prns_status_t status = prns_runtime_new(&runtime);
  if (status != PRNS_STATUS_OK || runtime == NULL) {
    return fail("runtime allocation failed", status);
  }

  uint64_t tick_count = UINT64_MAX;
  status = prns_runtime_tick_count(runtime, &tick_count);
  if (status != PRNS_STATUS_OK || tick_count != 0) {
    prns_runtime_free(runtime);
    return fail("initial tick count mismatch", status);
  }

  uint64_t emitted = UINT64_MAX;
  status = prns_runtime_tick(runtime, &emitted);
  if (status != PRNS_STATUS_OK || emitted != 0) {
    prns_runtime_free(runtime);
    return fail("tick mismatch", status);
  }

  status = prns_runtime_tick_count(runtime, &tick_count);
  if (status != PRNS_STATUS_OK || tick_count != 1) {
    prns_runtime_free(runtime);
    return fail("final tick count mismatch", status);
  }

  prns_runtime_free(runtime);
  puts("C_API_SMOKE_OK");
  return 0;
}
C

echo "[c] compile smoke consumer"
cc \
  -I personal-rns-capi/include \
  "${tmpdir}/smoke.c" \
  "${built_lib}" \
  -o "${tmpdir}/capi-smoke"

echo "[c] run smoke consumer"
env "${lib_env_name}=target/debug" "${tmpdir}/capi-smoke"

echo "CAPI_SMOKE_OK"
