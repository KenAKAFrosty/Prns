import { Tag } from "../../dist/casework.js";
import {
  PrnsProjectionStore,
  prnsView,
} from "../../dist/browser/projections.js";
import { WorkerProjectionSender } from "../../dist/browser/worker_projection_sender.js";
import { WireBatchDecoder } from "../../dist/worker_wire/wire_batch.js";

const publications = 10_000;
const store = new PrnsProjectionStore(snapshot(), Tag("Running"), 32);
const lifecycle = store.projection(prnsView("Lifecycle"));
let notifications = 0;
lifecycle.subscribe(() => {
  notifications += 1;
});
const localStarted = performance.now();
for (let index = 0; index < publications; index += 1) {
  store.replaceLifecycle(index % 2 === 0 ? Tag("Starting") : Tag("Running"));
}
const localIssuedMillis = performance.now() - localStarted;
await Promise.resolve();
const localSettledMillis = performance.now() - localStarted;

const channel = new MessageChannel();
const decoder = new WireBatchDecoder();
const received = nextMessage(channel.port2);
const sender = new WorkerProjectionSender(channel.port1, (detail) => {
  throw new Error(detail);
});
const wireStarted = performance.now();
for (let index = 0; index < publications; index += 1) {
  sender.send(Tag(
    "Lifecycle",
    {
      revision: BigInt(index + 1),
      value: index % 2 === 0 ? Tag("Starting") : Tag("Running"),
    },
  ));
}
const wireIssuedMillis = performance.now() - wireStarted;
const message = await received;
const updates = decoder.decode(message.data.batch);
const wireSettledMillis = performance.now() - wireStarted;
sender.receive(Tag("AcknowledgeProjection", { id: message.data.id }));
sender.close();
channel.port1.close();
channel.port2.close();

const result = {
  publications,
  local: {
    notifications,
    finalLifecycle: lifecycle.latest().value.tag,
    issuedMillis: localIssuedMillis,
    settledMillis: localSettledMillis,
  },
  wire: {
    frames: 1,
    frameKind: message.data.batch.tag,
    updates: updates.length,
    finalLifecycle: updates[0]?.data.value.tag,
    issuedMillis: wireIssuedMillis,
    settledMillis: wireSettledMillis,
  },
};
document.getElementById("result").textContent = JSON.stringify(result, null, 2);
await fetch("/browser-projections-result", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify(result),
});

function snapshot() {
  return {
    revision: 0n,
    backend: {
      backend: "Cooperative",
      capabilities: [],
      interfaceKinds: [],
    },
    interfaces: [],
    routes: [],
    activeLinkCount: 0,
    destinationIdentities: [],
    runtime: {
      running: true,
      uptimeMillis: 0,
      interfaceCount: 0,
      onlineInterfaceCount: 0,
      routeCount: 0,
      linkCount: 0,
      transportedLinkCount: 0,
      rxBytes: 0n,
      txBytes: 0n,
      rxBps: 0,
      txBps: 0,
    },
    persistence: {
      persistent: false,
      restored: false,
    },
  };
}

function nextMessage(port) {
  port.start();
  return new Promise((resolve) => {
    port.addEventListener("message", (event) => resolve(event.data), { once: true });
  });
}
