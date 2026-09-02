import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);
const packageDocument = JSON.parse(
  await readFile(new URL("package.json", root))
);
const productVersion = (
  await readFile(new URL("../../../VERSION", root), "utf8")
).trim();

test("locks the alternate package to the product release", () => {
  assert.equal(packageDocument.version, productVersion);
  assert.deepEqual(packageDocument.dependencies, {
    "personal-rns": productVersion
  });
});

test("contains only transparent module forwarding", async () => {
  const wrappers = {
    "index.js": "export * from \"personal-rns\";\n",
    "index.cjs": "module.exports = require(\"personal-rns\");\n",
    "index.d.ts": "export * from \"personal-rns\";\n",
    "native.js": "export * from \"personal-rns/native\";\n",
    "native.cjs": "module.exports = require(\"personal-rns/native\");\n",
    "native.d.ts": "export * from \"personal-rns/native\";\n",
    "browser.js": "export * from \"personal-rns/browser\";\n",
    "browser.d.ts": "export * from \"personal-rns/browser\";\n",
    "react.js": "export * from \"personal-rns/react\";\n",
    "react.d.ts": "export * from \"personal-rns/react\";\n",
    "solid.js": "export * from \"personal-rns/solid\";\n",
    "solid.d.ts": "export * from \"personal-rns/solid\";\n",
    "vue.js": "export * from \"personal-rns/vue\";\n",
    "vue.d.ts": "export * from \"personal-rns/vue\";\n",
    "svelte.js": "export * from \"personal-rns/svelte\";\n",
    "svelte.d.ts": "export * from \"personal-rns/svelte\";\n",
    "qwik.js": "export * from \"personal-rns/qwik\";\n",
    "qwik.d.ts": "export * from \"personal-rns/qwik\";\n",
    "web-component.js": "export * from \"personal-rns/web-component\";\n",
    "web-component.d.ts": "export * from \"personal-rns/web-component\";\n"
  };
  const contents = Object.fromEntries(
    await Promise.all(
      Object.keys(wrappers).map(async (path) => [
        path,
        await readFile(new URL(path, root), "utf8")
      ])
    )
  );
  assert.deepEqual(contents, wrappers);
});
