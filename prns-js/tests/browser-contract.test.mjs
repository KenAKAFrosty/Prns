import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

import {
  HOST_CONTRACT_ABI,
  DESTINATION_HASH_LENGTH,
  PRODUCT_VERSION,
  Prns,
  Tag,
  balancedLimits,
  match,
} from "personal-rns/browser";

test("browser subpath exposes the shared release contract and casework", () => {
  assert.equal(HOST_CONTRACT_ABI, 1);
  assert.equal(PRODUCT_VERSION, "0.3.1");
  assert.deepEqual(balancedLimits(), {
    pendingCommands: 256,
    applicationEvents: 1_024,
    retainedEventBytes: 8 * 1_024 * 1_024,
    diagnostics: 1_024,
  });
  assert.equal(typeof Prns.create, "function");
  assert.equal(
    match(Tag("Ready", 8), {
      Ready: (value) => value,
    }),
    8,
  );
});

test("generated JavaScript contract agrees with language-neutral vectors", async () => {
  const vectors = JSON.parse(
    await readFile("../prns-host/conformance/host-contract-v1.json", "utf8"),
  );
  assert.equal(HOST_CONTRACT_ABI, vectors.abi);
  assert.equal(PRODUCT_VERSION, vectors.productVersion);
  assert.equal(DESTINATION_HASH_LENGTH, vectors.fixedBytes.DestinationHash);
  assert.deepEqual(balancedLimits(), vectors.limits);
});
