import assert from "node:assert/strict";
import { test } from "node:test";

import {
  HOST_CONTRACT_ABI,
  PRODUCT_VERSION,
  Prns,
  Tag,
  balancedLimits,
  match,
} from "personal-rns/browser";

test("browser subpath exposes the shared release contract and casework", () => {
  assert.equal(HOST_CONTRACT_ABI, 1);
  assert.equal(PRODUCT_VERSION, "0.2.8");
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
