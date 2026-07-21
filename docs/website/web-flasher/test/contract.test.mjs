import assert from "node:assert/strict";
import test from "node:test";

import {
  BRIDGE_SCHEMA,
  testingContract,
  validateBridgeEvent,
} from "../src/contract.js";

test("the bridge contract has unique phase and error spellings", () => {
  assert.equal(BRIDGE_SCHEMA, 1);
  assert.equal(
    new Set(testingContract.phases.map((phase) => phase.wire)).size,
    testingContract.phases.length,
  );
  assert.equal(new Set(testingContract.errors).size, testingContract.errors.length);
  assert.equal(testingContract.phases.some((phase) => phase.wire === "success" && phase.terminal), true);
  assert.equal(testingContract.phases.some((phase) => phase.wire === "writing" && phase.busy), true);
});

test("events accept only contract-owned fields, phases, and errors", () => {
  assert.deepEqual(validateBridgeEvent({ schema: 1, phase: "writing", current: 1, total: 2 }), {
    schema: 1,
    phase: "writing",
    current: 1,
    total: 2,
  });
  assert.throws(() => validateBridgeEvent({ schema: 1, phase: "invented" }), /Bridge phase/);
  assert.throws(
    () => validateBridgeEvent({ schema: 1, phase: "failed", code: "invented" }),
    /Bridge error/,
  );
  assert.throws(
    () => validateBridgeEvent({ schema: 1, phase: "failed", password: "must-not-cross" }),
    /Bridge event field/,
  );
});
