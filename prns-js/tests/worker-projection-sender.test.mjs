import assert from "node:assert/strict";
import test from "node:test";

import { Tag } from "../dist/casework.js";
import { WorkerProjectionSender } from "../dist/browser/worker_projection_sender.js";
import { WireBatchDecoder } from "../dist/worker_wire/wire_batch.js";

test("coalesces projection tags into one cloned microtask frame", async () => {
  const channel = new MessageChannel();
  const failures = [];
  const sender = new WorkerProjectionSender(
    channel.port1,
    (detail) => failures.push(detail),
  );
  sender.send(Tag("Lifecycle", snapshot(1n, Tag("Starting"))));
  sender.send(Tag("Lifecycle", snapshot(2n, Tag("Running"))));
  sender.send(Tag("Interfaces", snapshot(2n, [])));
  const message = await nextMessage(channel.port2);
  assert.equal(message.tag, "ProjectionBatch");
  assert.equal(message.data.batch.tag, "ClonedBatch");
  assert.deepEqual(new WireBatchDecoder().decode(message.data.batch), [
    Tag("Lifecycle", snapshot(2n, Tag("Running"))),
    Tag("Interfaces", snapshot(2n, [])),
  ]);
  sender.receive(Tag("AcknowledgeProjection", { id: message.data.id }));
  assert.deepEqual(failures, []);
  sender.close();
  channel.port1.close();
  channel.port2.close();
});

test("holds the latest projection state until the in-flight frame is acknowledged", async () => {
  const channel = new MessageChannel();
  const failures = [];
  const sender = new WorkerProjectionSender(
    channel.port1,
    (detail) => failures.push(detail),
  );
  sender.send(Tag("Lifecycle", snapshot(1n, Tag("Starting"))));
  const first = await nextMessage(channel.port2);
  sender.send(Tag("Lifecycle", snapshot(2n, Tag("Stopping"))));
  await expectNoMessage(channel.port2);
  const secondMessage = nextMessage(channel.port2);
  sender.receive(Tag("AcknowledgeProjection", { id: first.data.id }));
  const second = await secondMessage;
  assert.deepEqual(new WireBatchDecoder().decode(second.data.batch), [
    Tag("Lifecycle", snapshot(2n, Tag("Stopping"))),
  ]);
  sender.receive(Tag("AcknowledgeProjection", { id: second.data.id }));
  assert.deepEqual(failures, []);
  sender.close();
  channel.port1.close();
  channel.port2.close();
});

test("composes diagnostic deltas while a projection frame is in flight", async () => {
  const channel = new MessageChannel();
  const sender = new WorkerProjectionSender(channel.port1, assert.fail);
  sender.send(Tag("Lifecycle", snapshot(1n, Tag("Running"))));
  const first = await nextMessage(channel.port2);
  sender.send(Tag("DiagnosticsDelta", {
    revision: 2n,
    dropped: 0,
    appended: [Tag("DiagnosticsDropped", { count: 1n })],
  }));
  sender.send(Tag("DiagnosticsDelta", {
    revision: 3n,
    dropped: 1,
    appended: [Tag("DiagnosticsDropped", { count: 2n })],
  }));
  const next = nextMessage(channel.port2);
  sender.receive(Tag("AcknowledgeProjection", { id: first.data.id }));
  const second = await next;
  assert.deepEqual(new WireBatchDecoder().decode(second.data.batch), [
    Tag("DiagnosticsDelta", {
      revision: 3n,
      dropped: 0,
      appended: [Tag("DiagnosticsDropped", { count: 2n })],
    }),
  ]);
  sender.close();
  channel.port1.close();
  channel.port2.close();
});

function snapshot(revision, value) {
  return { revision, value };
}

function nextMessage(port) {
  port.start();
  return new Promise((resolve) => {
    port.addEventListener("message", (event) => resolve(event.data), { once: true });
  });
}

async function expectNoMessage(port) {
  let delivered = false;
  const received = () => {
    delivered = true;
  };
  port.addEventListener("message", received);
  await new Promise((resolve) => setTimeout(resolve, 10));
  port.removeEventListener("message", received);
  assert.equal(delivered, false);
}
