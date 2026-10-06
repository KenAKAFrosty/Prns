import assert from "node:assert/strict";
import test from "node:test";
import { requestControllerEnrollment } from "../src/controller-enrollment.js";

const expected = {
  usb: { vendorId: 0x1209, productId: 1 }, manufacturer: "Stay Personal",
  product: "Personal Hopspot (SenseCAP Solar Node)", serialNumber: "PERSONAL-RNS-SOLARNODE-HOP",
  interfaceNumber: 0, request: 0x56, value: 0x5052, index: 0x4e53,
};
const request = { managedApplication: expected, controllerPublicKey: "ab".repeat(64), statusRequest: 0x57 };
const environment = { crypto: { getRandomValues(bytes) { bytes.set([4, 3, 2, 1]); } } };

function fixture(statuses = [1, 2]) {
  const calls = [];
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
      assert.deepEqual([...bytes], [4, 3, 2, 1, ...Array(64).fill(0xab)]);
      calls.push("request"); return { status: "ok", bytesWritten: 68 };
    },
    async controlTransferIn(control, length) {
      assert.equal(control.request, 0x57); assert.equal(length, 69); calls.push("poll");
      return { status: "ok", data: new DataView(Uint8Array.from([statuses.shift() ?? 1, 4, 3, 2, 1, ...Array(64).fill(0xcd)]).buffer) };
    },
  };
  const usb = { async requestDevice(options) {
    calls.push("picker");
    assert.deepEqual(options.filters, [{ vendorId: 0x1209, productId: 1, serialNumber: expected.serialNumber }]);
    return device;
  } };
  return { device, usb, calls, run: (options = request) => requestControllerEnrollment(usb, options, environment, { sleep: async () => {} }) };
}

test("waits for the matching durable receipt and closes USB", async () => {
  const f = fixture(); assert.equal(await f.run(), "cd".repeat(64));
  assert.deepEqual(f.calls, ["picker", "open", "claim", "request", "poll", "poll", "close"]);
});
test("wrong board and malformed keys cannot authorize anything", async () => {
  const f = fixture();
  await assert.rejects(f.run({ ...request, controllerPublicKey: "secret" }), /public key/);
  assert.deepEqual(f.calls, []);
  f.device.productName = "Other board";
  await assert.rejects(f.run(), /exact Personal Hopspot identity/);
  assert.deepEqual(f.calls, ["picker"]);
});
test("a rejected request is never success", async () => {
  const f = fixture(); f.device.controlTransferOut = async () => ({ status: "stall" });
  await assert.rejects(f.run(), /rejected/); assert.equal(f.calls.at(-1), "close");
});
test("a failed durable grant is never success", async () => {
  const f = fixture([3]); await assert.rejects(f.run(), /could not be saved/);
  assert.equal(f.calls.at(-1), "close");
});
test("a receipt for another transaction cannot confirm this request", async () => {
  const f = fixture();
  f.device.controlTransferIn = async () => ({ status: "ok", data: new DataView(Uint8Array.from([2, 9, 9, 9, 9, ...Array(64).fill(0xcd)]).buffer) });
  await assert.rejects(f.run(), /has not been confirmed/); assert.equal(f.calls.at(-1), "close");
});
test("disconnect and truncated responses cannot confirm a grant", async () => {
  const f = fixture();
  f.device.controlTransferIn = async () => ({ status: "ok", data: new DataView(new ArrayBuffer(4)) });
  await assert.rejects(f.run(), /Could not confirm/); assert.equal(f.calls.at(-1), "close");
  f.device.controlTransferIn = async () => { throw new Error("disconnected"); };
  await assert.rejects(f.run(), /disconnected/); assert.equal(f.calls.at(-1), "close");
});
