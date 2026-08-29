import assert from "node:assert/strict";
import test from "node:test";

import { Tag } from "../dist/casework.js";
import {
  PrnsProjectionCapacityError,
  prnsView,
} from "../dist/browser/index.js";
import { PrnsProjectionStore } from "../dist/browser/projections.js";
import { parseRuntimeProjectionSnapshot } from "../dist/browser/projection_snapshot.js";
import {
  parseProjectionSynchronization,
  parseWorkerProjectionUpdate,
} from "../dist/browser/worker_projection_validation.js";

const lifecycle = { tag: "Running", data: undefined };

function snapshot(revision = 1n, routes = []) {
  return {
    revision,
    backend: {
      backend: "Cooperative",
      capabilities: [],
      interfaceKinds: [],
    },
    interfaces: [],
    routes,
    activeLinkCount: 0,
    destinationIdentities: [],
    runtime: {
      running: true,
      uptimeMillis: 0,
      interfaceCount: 0,
      onlineInterfaceCount: 0,
      routeCount: routes.length,
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

test("caches equivalent projections and preserves identity until change", async () => {
  const store = new PrnsProjectionStore(snapshot(), lifecycle, 8);
  const left = store.projection(prnsView("Routes"));
  const right = store.projection(prnsView("Routes"));
  assert.equal(left.latest(), right.latest());
  const initial = left.latest();
  let notifications = 0;
  left.subscribe(() => {
    notifications += 1;
  });
  store.replaceHostSnapshot(snapshot(2n));
  await Promise.resolve();
  assert.equal(left.latest(), initial);
  assert.equal(notifications, 0);
  const route = {
    destination: new Uint8Array(16),
    hops: 1,
    interfaceId: new Uint8Array(16),
    learnedAtMillis: 1,
    lastRouteActivityAtMillis: 1,
    expiresAtMillis: 2,
  };
  store.replaceHostSnapshot(snapshot(3n, [route]));
  await Promise.resolve();
  assert.notEqual(left.latest(), initial);
  assert.deepEqual(left.latest().value, [route]);
  assert.equal(left.latest().revision, 3n);
  assert.equal(notifications, 1);
});

test("keeps diagnostics subscriber-bounded without deriving link state", async () => {
  const capacities = [];
  const store = new PrnsProjectionStore(snapshot(), lifecycle, 8, {
    diagnosticCapacityChanged: (capacity) => capacities.push(capacity),
  });
  const links = store.projection(prnsView("Links"));
  const diagnostics = store.projection(
    prnsView("Diagnostics", { maximumEvents: 2 }),
  );
  links.subscribe(() => undefined);
  const release = diagnostics.subscribe(() => undefined);
  const linkId = new Uint8Array(16);
  store.publishDiagnostic({
    tag: "LinkEstablished",
    data: { linkId, rttMillis: 7 },
  });
  store.publishDiagnostic({
    tag: "PeerIdentified",
    data: { linkId, identity: new Uint8Array(16) },
  });
  await Promise.resolve();
  assert.equal(links.latest().value.length, 0);
  assert.equal(diagnostics.latest().value.length, 2);
  assert.deepEqual(capacities, [2]);
  release();
  await Promise.resolve();
  assert.deepEqual(diagnostics.latest().value, []);
  assert.deepEqual(capacities, [2, 0]);
});

test("rejects diagnostic projection capacities outside the node limit", () => {
  const store = new PrnsProjectionStore(snapshot(), lifecycle, 8);
  assert.throws(
    () => store.projection(prnsView("Diagnostics", { maximumEvents: 9 })),
    PrnsProjectionCapacityError,
  );
});

test("accepts authoritative link snapshots and rejects stale revisions", async () => {
  const store = new PrnsProjectionStore(snapshot(), lifecycle, 8);
  const links = store.projection(prnsView("Links"));
  let notifications = 0;
  links.subscribe(() => {
    notifications += 1;
  });
  const link = {
    linkId: new Uint8Array(16).fill(7),
    rttMillis: 23,
  };
  store.replaceLinks([link], 5n);
  store.replaceLinks([], 4n);
  await Promise.resolve();
  assert.deepEqual(links.latest().value, [link]);
  assert.equal(links.latest().revision, 5n);
  assert.equal(notifications, 1);
});

test("publishes a locally derived view after another view advances the clock", () => {
  const store = new PrnsProjectionStore(snapshot(), lifecycle, 8);
  const interfaces = store.projection(prnsView("Interfaces"));
  const connected = {
    interfaceId: new Uint8Array(8),
    health: "Connected",
    rxBytes: 0n,
    txBytes: 0n,
    routeCount: 0,
    linkCount: 0,
    transportedLinkCount: 0,
  };
  store.replaceInterfaces([connected], 2n);
  store.replaceLifecycle(Tag("Stopping"));
  store.replaceInterfaces([{ ...connected, health: "Disabled" }]);
  assert.equal(interfaces.latest().value[0].health, "Disabled");
  assert.equal(interfaces.latest().revision, 4n);
});

test("observes on first subscription, releases on last, and synchronizes explicitly", async () => {
  const observed = [];
  const unobserved = [];
  const store = new PrnsProjectionStore(snapshot(), lifecycle, 8, {
    observed: (view) => observed.push(view.tag),
    unobserved: (view) => unobserved.push(view.tag),
    synchronize: async () => ({
      tag: "Synchronized",
      data: { revision: 9n, value: [] },
    }),
  });
  const routes = store.projection(prnsView("Routes"));
  const changed = () => undefined;
  const releaseLeft = routes.subscribe(changed);
  const releaseRight = routes.subscribe(changed);
  assert.deepEqual(observed, ["Routes"]);
  releaseLeft();
  assert.deepEqual(unobserved, []);
  releaseRight();
  assert.deepEqual(unobserved, ["Routes"]);
  const synchronized = await routes.synchronize();
  assert.equal(synchronized.tag, "Synchronized");
});

test("applies bounded diagnostic deltas without rebuilding link state", async () => {
  const store = new PrnsProjectionStore(snapshot(), lifecycle, 8);
  const diagnostics = store.projection(
    prnsView("Diagnostics", { maximumEvents: 3 }),
  );
  const release = diagnostics.subscribe(() => undefined);
  const first = { tag: "DiagnosticsDropped", data: { count: 1n } };
  const second = { tag: "DiagnosticsDropped", data: { count: 2n } };
  const third = { tag: "DiagnosticsDropped", data: { count: 3n } };
  store.replaceDiagnostics([first, second], 2n);
  store.appendDiagnostics(1, [third], 3n);
  store.appendDiagnostics(0, [first], 2n);
  assert.deepEqual(diagnostics.latest().value, [second, third]);
  assert.equal(diagnostics.latest().revision, 3n);
  release();
});

test("rejects a stale diagnostic delta after a local publication", () => {
  const store = new PrnsProjectionStore(snapshot(), lifecycle, 8);
  const diagnostics = store.projection(
    prnsView("Diagnostics", { maximumEvents: 3 }),
  );
  const release = diagnostics.subscribe(() => undefined);
  const local = { tag: "DiagnosticsDropped", data: { count: 1n } };
  const stale = { tag: "DiagnosticsDropped", data: { count: 2n } };
  store.publishDiagnostic(local);
  store.appendDiagnostics(0, [stale], 1n);
  assert.deepEqual(diagnostics.latest().value, [local]);
  release();
});

test("validates worker projection revisions and synchronized value shapes", () => {
  assert.throws(
    () => parseWorkerProjectionUpdate(Tag("Routes", { value: [] })),
    /revision/,
  );
  assert.throws(
    () => parseProjectionSynchronization(
      prnsView("Routes"),
      Tag("Synchronized", { revision: 2n, value: {} }),
    ),
    /array/,
  );
  assert.deepEqual(
    parseProjectionSynchronization(
      prnsView("Routes"),
      Tag("Synchronized", { revision: 2n, value: [] }),
    ),
    Tag("Synchronized", { revision: 2n, value: [] }),
  );
  assert.throws(
    () => parseWorkerProjectionUpdate(Tag("DiagnosticsReset", {
      revision: 2n,
      value: [Tag("FutureDiagnostic", {})],
    })),
    /unknown tag/,
  );
});

test("decodes narrow runtime projection snapshots", () => {
  const parsed = parseRuntimeProjectionSnapshot({
    revision: 4n,
    links: [{
      linkId: new Uint8Array(16).fill(1),
      rttMillis: 23,
      peerIdentity: new Uint8Array(16).fill(2),
    }],
  });
  assert.equal(parsed.revision, 4n);
  assert.equal(parsed.interfaces, undefined);
  assert.equal(parsed.routes, undefined);
  assert.equal(parsed.links.length, 1);
  assert.equal(parsed.links[0].rttMillis, 23);
});
