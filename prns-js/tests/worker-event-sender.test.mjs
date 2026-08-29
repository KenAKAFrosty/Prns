import assert from "node:assert/strict";
import test from "node:test";

import { Tag } from "../dist/casework.js";
import { BoundedWorkerEventSender } from "../dist/browser/worker_event_sender.js";

const singleDeliveryPayloadVector = Uint8Array.from([
  80, 82, 78, 69, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 12, 0, 0, 0, 100,
  0, 1, 0, 3, 0, 1, 0, 4, 0, 0, 0, 1, 2, 3, 4,
]);

const limits = {
  pendingCommands: 4,
  applicationEvents: 2,
  retainedEventBytes: 8,
  diagnostics: 1,
};

test("holds later event batches until the page acknowledges the in-flight batch", async () => {
  const channel = new MessageChannel();
  const failures = [];
  const sender = new BoundedWorkerEventSender(channel.port1, limits, {
    protocol: (detail) => failures.push({ type: "protocol", detail }),
    backpressure: (rejectedEventBytes) =>
      failures.push({ type: "backpressure", rejectedEventBytes }),
  });
  const firstMessage = nextMessage(channel.port2);
  sender.sendBatch(Uint8Array.from(singleDeliveryPayloadVector));
  sender.sendBatch(Uint8Array.from(singleDeliveryPayloadVector));
  const first = await firstMessage;
  assert.equal(first.tag, "Batch");
  const secondMessage = nextMessage(channel.port2);
  channel.port2.postMessage(Tag("Acknowledge", { id: first.data.id }));
  const second = await secondMessage;
  assert.equal(second.tag, "Batch");
  channel.port2.postMessage(Tag("Acknowledge", { id: second.data.id }));
  assert.deepEqual(failures, []);
  channel.port1.close();
  channel.port2.close();
});

test("coalesces diagnostics dropped behind a full event channel", async () => {
  const channel = new MessageChannel();
  const failures = [];
  const sender = new BoundedWorkerEventSender(channel.port1, limits, {
    protocol: (detail) => failures.push({ type: "protocol", detail }),
    backpressure: (rejectedEventBytes) =>
      failures.push({ type: "backpressure", rejectedEventBytes }),
  });
  const firstMessage = nextMessage(channel.port2);
  sender.sendDiagnostic(Tag("Delivered", { detail: "first" }));
  sender.sendDiagnostic(Tag("Delivered", { detail: "second" }));
  const first = await firstMessage;
  assert.deepEqual(first.data.event, Tag("Delivered", { detail: "first" }));
  const gapMessage = nextMessage(channel.port2);
  channel.port2.postMessage(Tag("Acknowledge", { id: first.data.id }));
  const gap = await gapMessage;
  assert.deepEqual(gap.data.event, Tag("DiagnosticsDropped", { count: 1n }));
  channel.port2.postMessage(Tag("Acknowledge", { id: gap.data.id }));
  assert.deepEqual(failures, []);
  channel.port1.close();
  channel.port2.close();
});

function nextMessage(port) {
  port.start();
  return new Promise((resolve) => {
    port.addEventListener("message", (event) => resolve(event.data), { once: true });
  });
}
