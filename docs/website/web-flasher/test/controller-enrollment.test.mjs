import assert from "node:assert/strict";
import test from "node:test";
import { requestControllerEnrollment } from "../src/controller-enrollment.js";
import { createEnrollmentWait } from "../src/enrollment-wait.js";

const expected = {
  usb: { vendorId: 0x1209, productId: 1 }, manufacturer: "Stay Personal",
  product: "Personal Hopspot (SenseCAP Solar Node)", serialNumber: "PERSONAL-RNS-SOLARNODE-HOP",
  interfaceNumber: 0, request: 0x56, value: 0x5052, index: 0x4e53,
};
const request = {
  managedApplication: expected, controllerPublicKey: "ab".repeat(64),
  statusRequest: 0x57, action: "authorize",
};
const checkRequest = { ...request, action: "check-status" };

function fixture(statuses = [0, 1, 2]) {
  const calls = [];
  const writes = [];
  const session = { operation: null };
  let transaction = 0;
  let randomCalls = 0;
  const environment = { crypto: { getRandomValues(bytes) {
    bytes.set([4 + randomCalls++, 3, 2, 1]);
  } } };
  const device = {
    vendorId: expected.usb.vendorId, productId: expected.usb.productId,
    manufacturerName: expected.manufacturer, productName: expected.product,
    serialNumber: expected.serialNumber, configuration: { interfaces: [{ interfaceNumber: 0 }] },
    opened: false,
    async open() { calls.push("open"); this.opened = true; },
    async close() { calls.push("close"); this.opened = false; },
    async claimInterface(index) { assert.equal(index, 0); calls.push("claim"); },
    async controlTransferOut(control, bytes) {
      assert.deepEqual(control, { requestType: "vendor", recipient: "device", request: 0x56, value: 0x5052, index: 0x4e53 });
      assert.deepEqual([...bytes.subarray(4)], Array(64).fill(0xab));
      transaction = new DataView(bytes.buffer).getUint32(0, true);
      writes.push(transaction); calls.push("request");
      return { status: "ok", bytesWritten: 68 };
    },
    async controlTransferIn(control, length) {
      assert.equal(control.request, 0x57); assert.equal(length, 69); calls.push("poll");
      const next = statuses.shift() ?? 1;
      if (next instanceof Error) throw next;
      const receipt = typeof next === "number" ? { status: next } : next;
      const bytes = new Uint8Array(69);
      const data = new DataView(bytes.buffer);
      data.setUint8(0, receipt.status);
      data.setUint32(1, receipt.transaction ?? transaction, true);
      bytes.fill(receipt.target ?? 0xcd, 5);
      return { status: "ok", data };
    },
  };
  const usb = { async requestDevice(options) {
    calls.push("picker");
    assert.deepEqual(options.filters, [{ vendorId: 0x1209, productId: 1, serialNumber: expected.serialNumber }]);
    return device;
  } };
  let elapsed = 0;
  const dependencies = { now: () => elapsed, sleep: async (ms) => { elapsed += ms; }, enrollmentSession: session };
  return {
    device, usb, calls, writes, statuses, session, dependencies,
    run: (options = request) => requestControllerEnrollment(usb, options, environment, dependencies),
  };
}

test("pins the board before sending and waits for its matching durable receipt", async () => {
  const f = fixture();
  assert.deepEqual(await f.run(), { status: "saved", targetPublicKey: "cd".repeat(64) });
  assert.deepEqual(f.writes, [0x01020304]);
  assert.deepEqual(f.calls, ["picker", "open", "claim", "poll", "request", "poll", "poll", "close"]);
});

test("wrong board, malformed keys and missing explicit action cannot authorize", async () => {
  const f = fixture();
  await assert.rejects(f.run({ ...request, controllerPublicKey: "secret" }), /public key/);
  await assert.rejects(f.run({ ...request, action: undefined }), /public key/);
  assert.deepEqual(f.calls, []);
  f.device.productName = "Other board";
  await assert.rejects(f.run(), /exact Personal Hopspot identity/);
  assert.deepEqual(f.calls, ["picker"]);
  assert.deepEqual(f.writes, []);
});

test("a busy board or unreadable preflight cannot receive a new grant", async () => {
  const f = fixture([1]);
  await assert.rejects(f.run(), /still saving/);
  f.device.controlTransferIn = async () => ({ status: "ok", data: new DataView(new ArrayBuffer(4)) });
  await assert.rejects(f.run(), /Could not read/);
  assert.deepEqual(f.writes, []);
  assert.equal(f.session.operation, null);
});

test("an explicit rejected request is never success", async () => {
  const f = fixture();
  f.device.controlTransferOut = async () => ({ status: "stall" });
  await assert.rejects(f.run(), /rejected/);
  assert.equal(f.calls.at(-1), "close");
});

test("a failed durable grant is never success", async () => {
  const f = fixture([0, 3]);
  await assert.rejects(f.run(), /could not be saved/);
  assert.equal(f.calls.at(-1), "close");
});

test("another transaction or target identity cannot confirm this request", async () => {
  for (const receipt of [
    { status: 2, transaction: 1234 },
    { status: 2, target: 0xee },
  ]) {
    const f = fixture([0, receipt]);
    assert.equal((await f.run()).status, "unknown");
    assert.equal(f.calls.at(-1), "close");
    assert.equal(f.writes.length, 1);
  }
});

test("a disconnected response remains unknown and reconnect only reads the retained transaction", async () => {
  const f = fixture([0, new Error("disconnected")]);
  assert.equal((await f.run()).status, "unknown");
  const original = f.session.operation.transaction;
  f.statuses.push(2);
  assert.deepEqual(await f.run(checkRequest), { status: "saved", targetPublicKey: "cd".repeat(64) });
  assert.equal(f.session.operation.transaction, original);
  assert.deepEqual(f.writes, [original]);
});

test("a lost write response is reconciled without a second write", async () => {
  const f = fixture([0]);
  const write = f.device.controlTransferOut.bind(f.device);
  f.device.controlTransferOut = async (...args) => {
    await write(...args);
    throw new Error("connection lost after acceptance");
  };
  assert.equal((await f.run()).status, "unknown");
  f.statuses.push(2);
  assert.equal((await f.run(checkRequest)).status, "saved");
  assert.equal(f.writes.length, 1);
});

test("stopping a paused check preserves pending; truncated recovery remains unknown", async () => {
  const f = fixture([0]);
  const wait = createEnrollmentWait({ ...f.dependencies, setupTimeoutMs: 500, onProgress(event) { if (event.paused) wait.stopChecking(); } });
  f.dependencies.enrollmentWait = wait;
  assert.equal((await f.run()).status, "pending");
  delete f.dependencies.enrollmentWait;
  const original = f.session.operation.transaction;
  f.device.controlTransferIn = async () => ({ status: "ok", data: new DataView(new ArrayBuffer(4)) });
  assert.equal((await f.run(checkRequest)).status, "unknown");
  assert.deepEqual(f.writes, [original]);
});

test("a cleared setup receipt never triggers automatic reauthorization", async () => {
  const f = fixture([0, 2]);
  assert.equal((await f.run()).status, "saved");
  f.statuses.push({ status: 0, transaction: 0 });
  assert.equal((await f.run(checkRequest)).status, "unknown");
  assert.deepEqual(f.writes, [0x01020304]);
});

test("a historical saved receipt can be checked without renewing controller access", async () => {
  const f = fixture([0, 2]);
  assert.equal((await f.run()).status, "saved");
  f.statuses.push(2);
  assert.equal((await f.run(checkRequest)).status, "saved");
  assert.deepEqual(f.writes, [0x01020304]);
});

test("reconnecting another board of the same model does not confirm or write", async () => {
  const f = fixture([0, new Error("disconnected")]);
  await f.run();
  f.statuses.push({ status: 2, target: 0xee });
  const result = await f.run(checkRequest);
  assert.equal(result.status, "unknown");
  assert.match(result.message, /different identity/);
  assert.equal(f.writes.length, 1);
});

test("checking without a matching retained operation does not open USB or authorize", async () => {
  const f = fixture();
  assert.equal((await f.run(checkRequest)).status, "unknown");
  assert.deepEqual(f.calls, []);
  await f.run();
  const count = f.calls.length;
  assert.equal((await f.run({ ...checkRequest, controllerPublicKey: "ee".repeat(64) })).status, "unknown");
  assert.equal(f.calls.length, count);
  assert.equal(f.writes.length, 1);
});

test("a separate explicit authorization starts a fresh transaction", async () => {
  const f = fixture([0, 2]);
  await f.run();
  f.statuses.push(2, 2);
  assert.equal((await f.run()).status, "saved");
  assert.deepEqual(f.writes, [0x01020304, 0x01020305]);
});

test("a definitive rejection cannot later be mistaken for a historical saved receipt", async () => {
  for (const stalled of [false, true]) {
    const f = fixture([0, 3]);
    if (stalled) f.device.controlTransferOut = async () => ({ status: "stall" });
    await assert.rejects(f.run(), /rejected|could not be saved/);
    const calls = f.calls.length;
    f.statuses.push(2);
    assert.equal((await f.run(checkRequest)).status, "unknown");
    assert.equal(f.calls.length, calls);
  }
});

test("fresh nRF transactions exclude both preflight and retained historical receipts", async () => {
  const f = fixture([{ status: 2, transaction: 0x01020304 }, 2]);
  assert.equal((await f.run()).status, "saved");
  assert.equal(f.writes[0], 0x01020305);
  // Next random word equals the retained transaction; the board reports another
  // historical receipt. Neither receipt may alias this new authorization.
  f.statuses.push({ status: 2, transaction: 0x01020306 }, 2);
  assert.equal((await f.run()).status, "saved");
  assert.equal(f.writes[1], 0x01020307);
});

test("nRF waits beyond slow startup and emits progress without another authorization", async () => {
  const f = fixture([0, ...Array(120).fill(1), 2]);
  const progress = [];
  f.dependencies.onProgress = event => progress.push(event);
  assert.equal((await f.run()).status, "saved");
  assert.ok(progress.length > 1);
  assert.ok(progress.every(event => !event.paused));
  assert.equal(f.writes.length, 1);
  assert.equal(f.calls.filter(call => call === "open").length, 1);
});

test("nRF paused continuation uses the same claimed interface and only reads status", async () => {
  const f = fixture([0, 1, 1, 1]);
  let paused;
  const didPause = new Promise(resolve => { paused = resolve; });
  const wait = createEnrollmentWait({ ...f.dependencies, setupTimeoutMs: 500,
    onProgress(event) { if (event.paused) paused(); },
  });
  f.dependencies.enrollmentWait = wait;
  const running = f.run();
  await didPause;
  const calls = f.calls.length;
  await Promise.resolve();
  assert.equal(f.calls.length, calls);
  assert.equal(f.device.opened, true);
  f.statuses.push(2);
  wait.continueChecking();
  assert.equal((await running).status, "saved");
  assert.equal(f.writes.length, 1);
  assert.equal(f.calls.filter(call => call === "open").length, 1);
  assert.equal(f.calls.filter(call => call === "close").length, 1);
});

test("leaving during nRF selection prevents authorization after the picker resolves", async () => {
  const f = fixture();
  const wait = createEnrollmentWait(f.dependencies);
  f.dependencies.enrollmentWait = wait;
  let selected, release;
  const selecting = new Promise(resolve => { selected = resolve; });
  f.usb.requestDevice = async () => {
    selected();
    await new Promise(resolve => { release = resolve; });
    return f.device;
  };
  const running = f.run();
  await selecting;
  wait.stopChecking();
  release();
  await assert.rejects(running, /stopped/);
  assert.equal(f.writes.length, 0);
  assert.equal(f.device.opened, false);
});
