import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const napiRoot = resolve(packageRoot, "../prns-napi");
const bindings = readdirSync(napiRoot)
  .filter((file) => file.endsWith(".node"))
  .sort();
assert.equal(bindings.length, 1, "exactly one local N-API binding is required");
process.env.NAPI_RS_NATIVE_LIBRARY_PATH = resolve(napiRoot, bindings[0]);

const esm = await import("personal-rns");
const require = createRequire(import.meta.url);
const commonjs = require("personal-rns");

test("root export selects one native API for ESM and CommonJS", () => {
  assert.equal(esm.HOST_CONTRACT_ABI, 1);
  assert.equal(esm.PRODUCT_VERSION, "0.2.8");
  assert.equal(commonjs.HOST_CONTRACT_ABI, esm.HOST_CONTRACT_ABI);
  assert.equal(commonjs.Prns, esm.Prns);
});

test("packaged native API starts, exposes lifecycle, and stops", async () => {
  const created = await esm.Prns.create({
    identity: esm.Tag("GenerateEphemeral"),
    role: "Endpoint",
  });
  assert.equal(created.tag, "Ready");
  assert.equal(created.data.lifecycle.tag, "Running");
  const events = created.data.claimEvents();
  assert.equal(events.tag, "Claimed");
  assert.deepEqual(created.data.claimEvents(), {
    tag: "AlreadyClaimed",
    data: { lane: "ApplicationEvents" },
  });
  const attached = await created.data.execute(
    esm.Tag("AttachTcpClient", {
      target: "127.0.0.1:9",
      bitrate: esm.Tag("Auto"),
    }),
  );
  assert.equal(attached.tag, "Succeeded");
  assert.equal(attached.data.tag, "InterfaceAttached");
  const detached = await created.data.execute(
    esm.Tag("DetachInterface", {
      interface: attached.data.data.interface,
    }),
  );
  assert.equal(detached.tag, "Succeeded");
  assert.equal(detached.data.tag, "InterfaceDetached");
  assert.equal((await created.data.stop()).tag, "Stopped");
  assert.equal(created.data.lifecycle.tag, "Stopped");
});
