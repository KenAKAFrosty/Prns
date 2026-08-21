import assert from "node:assert/strict";
import test from "node:test";

import {
  BRIDGE_SCHEMA,
  BridgeEventSequence,
  RESPONSE_LIMITS,
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
  assert.equal(
    new Set(testingContract.operations.map((operation) => operation.wire)).size,
    testingContract.operations.length,
  );
  assert.equal(testingContract.phases.some((phase) => phase.wire === "success" && phase.terminal), true);
  assert.equal(
    testingContract.phases.some((phase) => phase.wire === "download_requested" && phase.terminal),
    true,
  );
  assert.equal(testingContract.phases.some((phase) => phase.wire === "writing" && phase.busy), true);
  assert.deepEqual(RESPONSE_LIMITS, {
    channel_bytes: 64 * 1024,
    manifest_bytes: 512 * 1024,
    signature_bytes: 64 * 1024,
    artifact_bytes: 64 * 1024 * 1024,
  });
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

test("phase-owned error and progress shapes fail closed", () => {
  assert.throws(
    () => validateBridgeEvent({ schema: 1, phase: "connecting", code: "connection_failure" }),
    /cannot carry an error code/,
  );
  assert.throws(
    () => validateBridgeEvent({ schema: 1, phase: "failed", message: "recover" }),
    /require a non-cancellation error code/,
  );
  assert.throws(
    () => validateBridgeEvent({ schema: 1, phase: "cancelled", code: "cancelled" }),
    /require a recovery message/,
  );
  assert.throws(
    () => validateBridgeEvent({ schema: 1, phase: "writing", current: 2 }),
    /both current and total/,
  );
  assert.throws(
    () => validateBridgeEvent({ schema: 1, phase: "writing", current: 3, total: 2 }),
    /exceed total/,
  );
  assert.throws(
    () => validateBridgeEvent({ schema: 1, phase: "writing", current: 0, total: 0 }),
    /at least 1 byte/,
  );
  assert.throws(
    () => validateBridgeEvent({ schema: 1, phase: "ready", current: 1, total: 2, bytes: 2 }),
    /complete byte progress/,
  );
  assert.throws(
    () => validateBridgeEvent({
      schema: 1,
      phase: "downloading",
      current: 0,
      total: 2,
      part: "bootloader",
    }),
    /part, partIndex, and partCount together/,
  );
});

test("preparation sequences enforce transitions, monotonic progress, and one terminal event", () => {
  const sequence = new BridgeEventSequence("preparation");
  sequence.accept({ schema: 1, phase: "validating_manifest" });
  sequence.accept({
    schema: 1,
    phase: "downloading",
    current: 0,
    total: 4,
    part: "application",
    partIndex: 0,
    partCount: 1,
  });
  sequence.accept({
    schema: 1,
    phase: "verifying_artifacts",
    current: 4,
    total: 4,
    part: "application",
    partIndex: 0,
    partCount: 1,
  });
  sequence.accept({ schema: 1, phase: "ready", current: 4, total: 4, bytes: 4 });
  assert.equal(sequence.terminal, true);
  assert.throws(
    () => sequence.accept({ schema: 1, phase: "failed", code: "flash_failed", message: "recover" }),
    /after its terminal event/,
  );
});

test("device sequences reject skipped phases and unstable progress", () => {
  const skipped = new BridgeEventSequence("device");
  skipped.accept({ schema: 1, phase: "requesting_port" });
  assert.throws(
    () => skipped.accept({ schema: 1, phase: "writing", current: 0, total: 4 }),
    /transition requesting_port -> writing/,
  );

  const regressed = new BridgeEventSequence("device");
  regressed.accept({ schema: 1, phase: "requesting_port" });
  regressed.accept({ schema: 1, phase: "connecting" });
  regressed.accept({ schema: 1, phase: "verifying_target", detectedChip: "ESP32-S3" });
  regressed.accept({ schema: 1, phase: "writing", current: 3, total: 4 });
  assert.throws(
    () => regressed.accept({ schema: 1, phase: "writing", current: 2, total: 4 }),
    /moved backwards/,
  );
  assert.throws(
    () => regressed.accept({ schema: 1, phase: "writing", current: 4, total: 5 }),
    /total changed/,
  );
});

test("managed Nordic DFU has one typed user-gesture continuation", () => {
  const sequence = new BridgeEventSequence("device");
  sequence.accept({ schema: 1, phase: "requesting_port" });
  sequence.accept({ schema: 1, phase: "connecting" });
  sequence.accept({ schema: 1, phase: "awaiting_bootloader_port" });
  sequence.accept({
    schema: 1,
    phase: "verifying_target",
    detectedChip: "nRF52840 (2886:0057)",
  });
  sequence.accept({ schema: 1, phase: "writing", current: 0, total: 4 });
  sequence.accept({ schema: 1, phase: "writing", current: 4, total: 4 });
  sequence.accept({ schema: 1, phase: "verifying_flash", current: 4, total: 4 });
  sequence.accept({ schema: 1, phase: "resetting" });
  sequence.accept({ schema: 1, phase: "success", current: 4, total: 4 });
  assert.equal(sequence.terminal, true);
});

test("fresh device sequences require erasure before writing and cannot cancel after it begins", () => {
  const sequence = new BridgeEventSequence("device");
  sequence.accept({ schema: 1, phase: "requesting_port" });
  sequence.accept({ schema: 1, phase: "connecting" });
  sequence.accept({ schema: 1, phase: "verifying_target", detectedChip: "ESP32-S3" });
  sequence.accept({ schema: 1, phase: "erasing" });
  assert.throws(
    () => sequence.accept({
      schema: 1,
      phase: "cancelled",
      code: "cancelled",
      message: "stop",
    }),
    /transition erasing -> cancelled/,
  );
});

test("UF2 download requests terminate without using serial success semantics", () => {
  const sequence = new BridgeEventSequence("device");
  sequence.accept({
    schema: 1,
    phase: "download_requested",
    current: 4,
    total: 4,
  });
  assert.equal(sequence.terminal, true);
});
