import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";

import {
  Tag,
  from,
  match,
  match_into,
} from "../dist/casework.js";

const require = createRequire(import.meta.url);
const commonjs = require("../dist-cjs/casework.js");

test("ESM and CommonJS expose identical casework behavior", () => {
  const value = Tag("Active", { peers: 3 });
  assert.deepEqual(commonjs.Tag("Active", { peers: 3 }), value);
  assert.equal(
    match(value, {
      Active: ({ peers }) => peers,
    }),
    3,
  );
  assert.equal(
    match_into().from(value, {
      Active: ({ peers }) => peers,
    }),
    3,
  );
});

test("case constructors retain the declared tagged shape", () => {
  const tagged = from().MakeTag("Settled", { command: 4n });
  assert.deepEqual(tagged, {
    tag: "Settled",
    data: { command: 4n },
  });
});
