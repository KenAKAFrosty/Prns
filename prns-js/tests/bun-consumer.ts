import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const napiRoot = resolve(packageRoot, "../prns-napi");
const bindings = readdirSync(napiRoot)
  .filter((file) => file.endsWith(".node"))
  .sort();
assert.equal(bindings.length, 1);
process.env.NAPI_RS_NATIVE_LIBRARY_PATH = resolve(napiRoot, bindings[0]);

const prns = await import("personal-rns");
const created = await prns.Prns.create({
  identity: prns.Tag("GenerateEphemeral"),
  role: "Endpoint",
});
assert.equal(created.tag, "Ready");
assert.equal(created.data.lifecycle.tag, "Running");
assert.equal((await created.data.stop()).tag, "Stopped");
