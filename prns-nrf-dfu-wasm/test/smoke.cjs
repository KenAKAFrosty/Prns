"use strict";

const assert = require("node:assert/strict");
const path = require("node:path");

const modulePath = process.argv[2];
if (!modulePath) {
  throw new Error("usage: node smoke.cjs PATH_TO_GENERATED_NODE_BINDING");
}
const bindings = require(path.resolve(modulePath));

const compatibility = bindings.NrfDfuCompatibility.notEnforcedApplication(
  0x0052,
  52840,
  Uint16Array.of(0x0123),
  bindings.NrfDfuBankLayout.Single,
);
const session = new bindings.NrfDfuSession(
  Uint8Array.of(1, 2, 3),
  Uint8Array.of(
    0x52,
    0x00,
    0x68,
    0xce,
    0xff,
    0xff,
    0xff,
    0xff,
    0x01,
    0x00,
    0x23,
    0x01,
    0xad,
    0xad,
  ),
  compatibility,
);

assert.equal(session.state, bindings.NrfDfuSessionState.Ready);
const first = session.nextFrame();
assert.equal(first.attempt, 1);
assert.equal(first.attemptLimit, 3);
assert.ok(first.bytes instanceof Uint8Array);
assert.ok(first.bytes.byteLength > 0);
assert.equal(session.state, bindings.NrfDfuSessionState.AwaitingAcknowledgement);

const awaiting = session.pushAcknowledgement(Uint8Array.of(0xc0, 0x10));
assert.equal(
  awaiting.kind,
  bindings.NrfDfuAcknowledgementTransitionKind.AwaitingMore,
);
const accepted = session.pushAcknowledgement(Uint8Array.of(0x00, 0x00, 0xf0, 0xc0));
assert.equal(
  accepted.kind,
  bindings.NrfDfuAcknowledgementTransitionKind.FrameAccepted,
);
assert.equal(accepted.waitMilliseconds, 500);
assert.equal(accepted.writtenBytes, 0);
assert.equal(accepted.totalBytes, 3);
assert.equal(session.state, bindings.NrfDfuSessionState.Ready);

const second = session.nextFrame();
const retryRequired = session.pushAcknowledgement(
  Uint8Array.of(0xc0, 0x10, 0x00, 0x00, 0x00, 0xc0),
);
assert.equal(
  retryRequired.kind,
  bindings.NrfDfuAcknowledgementTransitionKind.RetryRequired,
);
assert.equal(
  retryRequired.retryReason(),
  bindings.NrfDfuRetryReason.MalformedAcknowledgement,
);
assert.equal(session.state, bindings.NrfDfuSessionState.RetryRequired);
assert.throws(
  () => session.pushAcknowledgement(Uint8Array.of(0xc0)),
  /must be retransmitted/,
);
const retry = session.retryFrame();
assert.equal(second.attempt, 1);
assert.equal(retry.attempt, 2);
assert.deepEqual(retry.bytes, second.bytes);

retry.free();
retryRequired.free();
second.free();
accepted.free();
awaiting.free();
first.free();
session.free();
compatibility.free();
