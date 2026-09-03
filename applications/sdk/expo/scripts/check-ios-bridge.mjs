import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const swift = readFileSync(resolve(packageRoot, "ios/PrnsAppModule.swift"), "utf8");
const podspec = readFileSync(resolve(packageRoot, "ios/PrnsApp.podspec"), "utf8");

assert.match(
  swift,
  /DispatchQueue\(\s*label: "rs\.reticulum\.prns\.app\.native",\s*attributes: \.concurrent\s*\)/,
  "native calls must run on a concurrent queue so Rust owns command admission",
);
assert.equal(
  swift.match(/\.runOnQueue\(Self\.nativeQueue\)/g)?.length,
  21,
  "every Expo bridge function must use the native operation queue",
);
assert.match(
  swift,
  /nativeQueue\.sync\(flags: \.barrier\)/,
  "module teardown must wait for in-flight native calls before stopping Rust",
);
assert.match(swift, /\.appendingPathComponent\("prns", isDirectory: true\)/);
assert.match(swift, /\.appendingPathComponent\("development", isDirectory: true\)/);

for (const abiName of [
  "prns_app_contract_fingerprint",
  "prns_app_host_contract_fingerprint",
  "prns_app_inspect_identity",
  "prns_app_preview_identity_import",
  "prns_app_create_generated_identity",
  "prns_app_create_imported_identity",
  "prns_app_start",
  "prns_app_snapshot",
  "prns_app_initiate_pairing",
  "prns_app_approve_pairing",
  "prns_app_reject_pairing",
  "prns_app_describe_target",
  "prns_app_save_observed_destination",
  "prns_app_create_manual_contact",
  "prns_app_set_contact_alias",
  "prns_app_set_contact_pinned",
  "prns_app_delete_contact",
  "prns_app_get_contact",
  "prns_app_list_contacts",
  "prns_app_stop",
  "prns_app_reset",
  "prns_app_bytes_free",
]) {
  assert.match(swift, new RegExp(`\\b${abiName}\\b`), `Swift bridge must call ${abiName}`);
}

assert.match(
  podspec,
  /"\$\{PODS_ROOT\}\/\.\.\/\.\.\/\.\.\/native-composition\/include"/,
  "the generated app target must be able to import the public C bridge header",
);
assert.match(
  podspec,
  /"\$\{PODS_CONFIGURATION_BUILD_DIR\}"/,
  "the app target and Rust build phase must share one archive directory",
);

console.log(
  "ios:check: concurrent native calls, barrier teardown, ABI, storage, and linkage are exact",
);
