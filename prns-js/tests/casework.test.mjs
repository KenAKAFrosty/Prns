import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";

import {
  Tag,
  from,
  match,
  match_into,
} from "personal-rns/casework";

const require = createRequire(import.meta.url);
const commonjs = require("personal-rns/casework");

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

test("undeclared runtime tags fail closed", () => {
  assert.throws(
    () => match({ tag: "Future", data: undefined }, {}),
    /outside its declared union/,
  );
});
