import assert from "node:assert/strict";
import test from "node:test";

import { Tag } from "../dist/casework.js";
import { prnsView } from "../dist/browser/projections.js";
import { WorkerProjectionServer } from "../dist/browser/worker_projection_server.js";
import { WireBatchDecoder } from "../dist/worker_wire/wire_batch.js";

test("observes on demand, releases, and synchronizes without retaining", async () => {
  const channel = new MessageChannel();
  const state = projectionState();
  new WorkerProjectionServer(channel.port1, state.engine);
  channel.port2.start();
  const observedMessage = nextMessage(channel.port2);
  channel.port2.postMessage(Tag("Observe", { view: prnsView("Routes") }));
  const observed = await observedMessage;
  assert.equal(observed.tag, "ProjectionBatch");
  assert.deepEqual(new WireBatchDecoder().decode(observed.data.batch), [
    Tag("Routes", { revision: 1n, value: [] }),
  ]);
  assert.equal(state.subscriptions, 2);
  channel.port2.postMessage(Tag("AcknowledgeProjection", {
    id: observed.data.id,
  }));
  channel.port2.postMessage(Tag("Unobserve", { view: prnsView("Routes") }));
  const synchronizedMessage = nextMessage(channel.port2);
  channel.port2.postMessage(Tag("Synchronize", {
    id: 7,
    view: prnsView("Routes"),
  }));
  const synchronized = await synchronizedMessage;
  assert.deepEqual(synchronized, Tag("ProjectionSynchronized", {
    id: 7,
    outcome: Tag("Synchronized", { revision: 1n, value: [] }),
  }));
  assert.equal(state.releases, 1);
  assert.equal(state.subscriptions, 2);
  channel.port1.close();
  channel.port2.close();
});

test("replicates lifecycle changes without page demand", async () => {
  const channel = new MessageChannel();
  const state = projectionState();
  new WorkerProjectionServer(channel.port1, state.engine);
  channel.port2.start();

  const changed = nextMessage(channel.port2);
  state.publish(Tag("Failed", {
    cause: "ContractViolated",
    detail: "projection contract failed",
  }));
  const message = await changed;

  assert.equal(message.tag, "ProjectionBatch");
  assert.deepEqual(new WireBatchDecoder().decode(message.data.batch), [
    Tag("Lifecycle", {
      revision: 2n,
      value: Tag("Failed", {
        cause: "ContractViolated",
        detail: "projection contract failed",
      }),
    }),
  ]);
  channel.port2.postMessage(Tag("AcknowledgeProjection", {
    id: message.data.id,
  }));
  channel.port1.close();
  channel.port2.close();
});

test("bounds concurrent projection synchronization", async () => {
  const channel = new MessageChannel();
  const never = new Promise(() => undefined);
  const engine = {
    projection: () => ({
      latest: () => ({ revision: 1n, value: [] }),
      subscribe: () => () => undefined,
      synchronize: () => never,
    }),
  };
  new WorkerProjectionServer(channel.port1, engine);
  channel.port2.start();
  const busy = nextMessage(channel.port2);
  for (let id = 1; id <= 33; id += 1) {
    channel.port2.postMessage(Tag("Synchronize", {
      id,
      view: prnsView("Routes"),
    }));
  }
  assert.deepEqual(await busy, Tag("ProjectionSynchronized", {
    id: 33,
    outcome: Tag("Busy"),
  }));
  channel.port1.close();
  channel.port2.close();
});

test("rejects malformed projection requests as protocol failures", async () => {
  const channel = new MessageChannel();
  const state = projectionState();
  new WorkerProjectionServer(channel.port1, state.engine);
  channel.port2.start();
  const failure = nextMessage(channel.port2);
  channel.port2.postMessage(Tag("Synchronize", {
    id: 0,
    view: prnsView("Routes"),
  }));
  const message = await failure;
  assert.equal(message.tag, "ProjectionProtocolFailed");
  assert.match(message.data.detail, /positive safe integer/);
  channel.port1.close();
  channel.port2.close();
});

function projectionState() {
  const state = {
    subscriptions: 0,
    releases: 0,
    revision: 1n,
    value: [],
    changed: new Set(),
    engine: undefined,
    publish(value) {
      this.revision += 1n;
      this.value = value;
      for (const changed of this.changed) {
        changed();
      }
    },
  };
  const projection = {
    latest: () => ({ revision: state.revision, value: state.value }),
    subscribe: (changed) => {
      state.subscriptions += 1;
      state.changed.add(changed);
      return () => {
        state.releases += 1;
        state.changed.delete(changed);
      };
    },
    synchronize: async () => Tag("Synchronized", {
      revision: 1n,
      value: [],
    }),
  };
  state.engine = { projection: () => projection };
  return state;
}

function nextMessage(port) {
  return new Promise((resolve) => {
    port.addEventListener("message", (event) => resolve(event.data), {
      once: true,
    });
  });
}
