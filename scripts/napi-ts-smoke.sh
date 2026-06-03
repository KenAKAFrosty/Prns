#!/usr/bin/env bash
# Smoke-test the local @personal/rns TypeScript package scaffold.
#
# The compiled .node artifact stays uncommitted. This script builds the napi-rs
# crate, lays out a temp node_modules/@personal/rns package, and imports it from
# TypeScript so the packaging shape stays honest as the API grows.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v node >/dev/null 2>&1; then
  echo "napi-ts-smoke: node is required" >&2
  exit 1
fi

if ! node -e 'process.exit(process.versions.napi ? 0 : 1)' >/dev/null 2>&1; then
  echo "napi-ts-smoke: current node does not report Node-API support" >&2
  exit 1
fi

tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/prns-napi-package-smoke.XXXXXX")"
trap 'rm -rf "$tmpdir"' EXIT

package_src="personal-rns-napi"

echo "[rust] build personal-rns-napi addon"
cargo build -p personal-rns-napi

case "$(uname -s)" in
  Darwin)
    built_lib="target/debug/libpersonal_rns_napi.dylib"
    ;;
  Linux)
    built_lib="target/debug/libpersonal_rns_napi.so"
    ;;
  *)
    echo "napi-ts-smoke: unsupported host $(uname -s)" >&2
    exit 1
    ;;
esac

package_dir="${tmpdir}/node_modules/@personal/rns"
mkdir -p "${package_dir}"
cp "${package_src}/package.json" "${package_dir}/package.json"
cp "${package_src}/index.js" "${package_dir}/index.js"
cp "${package_src}/index.d.ts" "${package_dir}/index.d.ts"
cp "${built_lib}" "${package_dir}/personal_rns_napi.node"

cat >"${tmpdir}/smoke.ts" <<'TYPESCRIPT'
const addon = require("@personal/rns") as typeof import("@personal/rns");

const runtime = new addon.ReticulumRuntime();
const initialTickCount: bigint = runtime.tickCount();
const emitted: bigint = runtime.tick();
const finalTickCount: bigint = runtime.tickCount();
const linkedVersion: string = addon.version();

if (!linkedVersion) {
  throw new Error("expected non-empty personal-rns-ffi version");
}
if (initialTickCount !== 0n) {
  throw new Error(`expected initial tick count 0, got ${initialTickCount}`);
}
if (emitted !== 0n) {
  throw new Error(`expected no emitted packets, got ${emitted}`);
}
if (finalTickCount !== 1n) {
  throw new Error(`expected final tick count 1, got ${finalTickCount}`);
}

console.log(`@personal/rns napi-rs smoke ok: version=${linkedVersion} ticks=${finalTickCount}`);
TYPESCRIPT

echo "[node] run TypeScript smoke"
node "${tmpdir}/smoke.ts"

echo "NAPI_RS_TS_SMOKE_OK"
