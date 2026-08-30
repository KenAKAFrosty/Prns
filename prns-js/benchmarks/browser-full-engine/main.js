import {
  Prns,
  prnsView,
} from "/prns-js/dist/browser/index.js";

const REPETITIONS = 5;
const PAYLOAD_BYTES = 64;
let nextRunId = 1;
const workloads = [
  { name: "sequential", commands: 40, grain: 1 },
  { name: "coalesced", commands: 1_000, grain: 100 },
  { name: "bounded", commands: 1_024, grain: 256 },
];
const contentionWorkload = {
  packets: 128,
  snapshots: 4_096,
  snapshotGrain: 32,
};

run().catch(reportFailure);

async function run() {
  await progress("session");
  const session = await loadSession();
  await progress("target");
  const target = await prepareTarget(session.webSocketUrl);
  await progress("engine-worker");
  const engineWorker = await prepare(
    "EngineWorker",
    session.webSocketUrl,
    target.destination,
  );
  await progress("network-worker");
  const networkWorker = await prepare(
    "NetworkWorker",
    session.webSocketUrl,
    target.destination,
  );
  const configurations = [engineWorker, networkWorker];
  const payloads = Array.from(
    { length: Math.max(...workloads.map((workload) => workload.commands)) },
    (_, index) => payload(index),
  );
  const measurements = Object.fromEntries(
    workloads.map((workload) => [
      workload.name,
      new Map(configurations.map((configuration) => [configuration.execution, []])),
    ]),
  );
  const contentionMeasurements = new Map(
    configurations.map((configuration) => [configuration.execution, []]),
  );
  try {
    for (const configuration of configurations) {
      await progress(`warmup-${configuration.execution}`);
      await commandRun(configuration, workloads[1], payloads, target.deliveries);
    }
    for (let repetition = 0; repetition < REPETITIONS; repetition += 1) {
      const order = repetition % 2 === 0
        ? configurations
        : [...configurations].reverse();
      for (const workload of workloads) {
        for (const configuration of order) {
          await progress(`${repetition}-${workload.name}-${configuration.execution}`);
          measurements[workload.name].get(configuration.execution).push(
            await commandRun(
              configuration,
              workload,
              payloads,
              target.deliveries,
            ),
          );
        }
      }
      for (const configuration of order) {
        await progress(`${repetition}-contention-${configuration.execution}`);
        contentionMeasurements.get(configuration.execution).push(
          await contentionRun(
            configuration,
            contentionWorkload,
            payloads,
            target.deliveries,
          ),
        );
      }
    }
    const result = {
      userAgent: navigator.userAgent,
      repetitions: REPETITIONS,
      payloadBytes: PAYLOAD_BYTES,
      journey: configurations.map((configuration) => configuration.journey),
      workloads: workloads.map((workload) => ({
        ...workload,
        results: configurations.map((configuration) => summarize(
          configuration.execution,
          workload.commands,
          measurements[workload.name].get(configuration.execution),
        )),
      })),
      contention: {
        ...contentionWorkload,
        results: configurations.map((configuration) => summarizeContention(
          configuration.execution,
          contentionMeasurements.get(configuration.execution),
        )),
      },
    };
    document.getElementById("result").textContent = JSON.stringify(result, null, 2);
    await Promise.all(configurations.map((configuration) => configuration.stop()));
    await target.stop();
    await fetch("/browser-full-engine-result", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(result),
    });
  } catch (error) {
    await Promise.allSettled([
      ...configurations.map((configuration) => configuration.stop()),
      target.stop(),
    ]);
    throw error;
  }
}

async function prepare(execution, webSocketUrl, destination) {
  const started = performance.now();
  const created = await Prns.create({
    execution: "DedicatedWorker",
    networkExecution: execution,
    wasmModuleUrl: new URL("/prns-js/wasm/prns_wasm.js", location.href),
  });
  const startupMillis = performance.now() - started;
  if (created.tag !== "Ready") {
    throw new Error(`${execution} startup failed: ${created.tag}`);
  }
  const prns = created.data;
  const interfaces = prns.projection(prnsView("Interfaces"));
  const routes = prns.projection(prnsView("Routes"));
  const links = prns.projection(prnsView("Links"));
  const diagnostics = prns.projection(prnsView("Diagnostics", { maximumEvents: 64 }));
  let releases = [interfaces, routes, links, diagnostics].map((projection) =>
    projection.subscribe(() => undefined)
  );
  const connected = await measure(() =>
    prns.interfaces.webSocket.connect(relayPeerUrl(webSocketUrl, execution))
  );
  requireTag(connected.value, "Connected", `${execution} WebSocket connection`);
  const discovered = await measure(() => prns.requestPath(destination));
  if (discovered.value.tag !== "Succeeded") {
    throw new Error(`${execution} path debug: ${JSON.stringify({
      lifecycle: prns.lifecycle,
      settlement: discovered.value,
      hostSnapshot: await prns.hostSnapshot(),
      diagnostics: diagnostics.latest(),
    }, (_key, value) => typeof value === "bigint" ? value.toString() : value)}`);
  }
  requireSettlement(discovered.value, "PathDiscovered", `${execution} path request`);
  const established = await measure(() => prns.establishLink(destination));
  const link = requireSettlement(established.value, "LinkEstablished", `${execution} link`);
  await waitFor(() =>
    interfaces.latest().value.length === 1 &&
    routes.latest().value.length >= 1 &&
    links.latest().value.length === 1 &&
    diagnostics.latest().value.length >= 1
  );
  for (const release of releases) {
    release();
  }
  releases = [];
  return {
    execution,
    linkId: link.data.data.linkId,
    prns,
    journey: {
      execution,
      startupMillis,
      connectMillis: connected.millis,
      pathMillis: discovered.millis,
      linkMillis: established.millis,
      interfaces: interfaces.latest().value.length,
      routes: routes.latest().value.length,
      links: links.latest().value.length,
      diagnostics: diagnostics.latest().value.length,
    },
    setProjectionObservation(observed) {
      for (const release of releases) {
        release();
      }
      releases = observed
        ? [interfaces, routes, links, diagnostics].map((projection) =>
            projection.subscribe(() => undefined)
          )
        : [];
    },
    async stop() {
      for (const release of releases) {
        release();
      }
      await prns.stop();
    },
  };
}

async function prepareTarget(webSocketUrl) {
  const created = await Prns.create({
    execution: "DedicatedWorker",
    networkExecution: "EngineWorker",
    wasmModuleUrl: new URL("/prns-js/wasm/prns_wasm.js", location.href),
  });
  requireTag(created, "Ready", "target startup");
  const prns = created.data;
  const registered = await prns.registerSingleDestination({
    appName: "prns-browser-lab",
    aspects: ["worker-network-target"],
    requestHandlers: [],
  });
  requireTag(registered, "Registered", "target destination registration");
  const claimed = prns.claimEvents();
  requireTag(claimed, "Claimed", "target application event claim");
  const deliveries = deliveryTracker(claimed.data);
  const connected = await prns.interfaces.webSocket.connect(
    relayPeerUrl(webSocketUrl, "Target"),
  );
  requireTag(connected, "Connected", "target WebSocket connection");
  return {
    destination: registered.data,
    deliveries,
    async stop() {
      await prns.stop();
      await deliveries.finished;
    },
  };
}

async function commandRun(configuration, workload, payloads, deliveries) {
  const observer = taskDelayObserver();
  const runId = nextRunId;
  nextRunId += 1;
  const runPayloads = payloadsForRun(payloads, workload.commands, runId);
  const delivered = deliveries.expect(runId, workload.commands);
  let submissionMillis = 0;
  const started = performance.now();
  for (let offset = 0; offset < workload.commands; offset += workload.grain) {
    const end = Math.min(offset + workload.grain, workload.commands);
    const submissionStarted = performance.now();
    const pending = [];
    for (let index = offset; index < end; index += 1) {
      pending.push(configuration.prns.sendLinkPacket(
        configuration.linkId,
        runPayloads[index],
      ));
    }
    submissionMillis += performance.now() - submissionStarted;
    const outcomes = await Promise.all(pending);
    for (const outcome of outcomes) {
      requireSettlement(outcome, "PacketDelivered", `${configuration.execution} link packet`);
    }
  }
  const settlementsMillis = performance.now() - started;
  const delivery = await delivered;
  const elapsedMillis = performance.now() - started;
  return {
    elapsedMillis,
    settlementsMillis,
    firstDeliveryMillis: delivery.firstAt - started,
    deliveryMillis: delivery.completedAt - started,
    submissionMillis,
    maximumEventLoopGapMillis: await observer.stop(),
  };
}

async function contentionRun(configuration, workload, payloads, deliveries) {
  const runId = nextRunId;
  nextRunId += 1;
  const runPayloads = payloadsForRun(payloads, workload.packets, runId);
  const delivered = deliveries.expect(runId, workload.packets);
  const relayMeasurement = await startRelayMeasurement(
    configuration.execution,
    workload.packets,
  );
  const observer = taskDelayObserver();
  const started = performance.now();
  const packetPromises = runPayloads.map((payload) =>
    configuration.prns.sendLinkPacket(configuration.linkId, payload)
  );
  const packetCompletion = Promise.all(packetPromises).then((outcomes) => ({
    outcomes,
    completedAt: performance.now(),
  }));
  const snapshotCompletion = runSnapshotContention(
    configuration,
    workload,
  );
  const [packets, snapshots, delivery] = await Promise.all([
    packetCompletion,
    snapshotCompletion,
    delivered,
  ]);
  for (const outcome of packets.outcomes) {
    requireSettlement(
      outcome,
      "PacketDelivered",
      `${configuration.execution} contention link packet`,
    );
  }
  const relay = await waitForRelayMeasurement(relayMeasurement.id);
  return {
    totalMillis: performance.now() - started,
    packetSettlementMillis: packets.completedAt - started,
    snapshotMillis: snapshots.completedAt - started,
    firstDeliveryMillis: delivery.firstAt - started,
    deliveryMillis: delivery.completedAt - started,
    relayFirstFrameMillis: relay.firstMillis,
    relayCompleteMillis: relay.completeMillis,
    maximumEventLoopGapMillis: await observer.stop(),
  };
}

async function runSnapshotContention(configuration, workload) {
  for (let offset = 0; offset < workload.snapshots; offset += workload.snapshotGrain) {
    const count = Math.min(
      workload.snapshotGrain,
      workload.snapshots - offset,
    );
    const outcomes = await Promise.all(Array.from(
      { length: count },
      () => configuration.prns.snapshot(),
    ));
    for (const outcome of outcomes) {
      requireTag(
        outcome,
        "Captured",
        `${configuration.execution} contention snapshot`,
      );
    }
  }
  return { completedAt: performance.now() };
}

function summarize(execution, commands, samples) {
  const elapsedMillis = median(samples.map((sample) => sample.elapsedMillis));
  return {
    execution,
    elapsedMillis,
    submissionMillis: median(samples.map((sample) => sample.submissionMillis)),
    settlementsMillis: median(samples.map((sample) => sample.settlementsMillis)),
    firstDeliveryMillis: median(samples.map((sample) => sample.firstDeliveryMillis)),
    deliveryMillis: median(samples.map((sample) => sample.deliveryMillis)),
    maximumEventLoopGapMillis: median(
      samples.map((sample) => sample.maximumEventLoopGapMillis),
    ),
    commandsPerSecond: commands / (elapsedMillis / 1_000),
  };
}

function summarizeContention(execution, samples) {
  return {
    execution,
    totalMillis: median(samples.map((sample) => sample.totalMillis)),
    packetSettlementMillis: median(
      samples.map((sample) => sample.packetSettlementMillis),
    ),
    snapshotMillis: median(samples.map((sample) => sample.snapshotMillis)),
    firstDeliveryMillis: median(
      samples.map((sample) => sample.firstDeliveryMillis),
    ),
    deliveryMillis: median(samples.map((sample) => sample.deliveryMillis)),
    relayFirstFrameMillis: median(
      samples.map((sample) => sample.relayFirstFrameMillis),
    ),
    relayCompleteMillis: median(
      samples.map((sample) => sample.relayCompleteMillis),
    ),
    maximumEventLoopGapMillis: median(
      samples.map((sample) => sample.maximumEventLoopGapMillis),
    ),
  };
}

function taskDelayObserver() {
  let maximum = 0;
  let previous = performance.now();
  const timer = setInterval(() => {
    const current = performance.now();
    maximum = Math.max(maximum, current - previous);
    previous = current;
  }, 1);
  return {
    async stop() {
      await new Promise((resolve) => setTimeout(resolve, 0));
      clearInterval(timer);
      return maximum;
    },
  };
}

function payload(index) {
  const bytes = Uint8Array.from(
    { length: PAYLOAD_BYTES },
    (_, byte) => (index * 31 + byte) & 0xff,
  );
  new DataView(bytes.buffer).setUint32(4, index, true);
  return bytes;
}

function payloadsForRun(payloads, count, runId) {
  return payloads.slice(0, count).map((payload) => {
    const bytes = payload.slice();
    new DataView(bytes.buffer).setUint32(0, runId, true);
    return bytes;
  });
}

function deliveryTracker(events) {
  const pending = new Map();
  let failure;
  const finished = consume();
  return {
    finished,
    expect(runId, count) {
      if (failure !== undefined) {
        return Promise.reject(failure);
      }
      if (pending.has(runId)) {
        return Promise.reject(new Error(`duplicate delivery run ${runId}`));
      }
      return new Promise((resolve, reject) => {
        pending.set(runId, {
          count,
          seen: new Uint8Array(count),
          received: 0,
          firstAt: undefined,
          resolve,
          reject,
        });
      });
    },
  };

  async function consume() {
    try {
      for await (const event of events) {
        if (event.tag !== "LinkDelivery") {
          continue;
        }
        receive(event.data.plaintext);
      }
    } catch (error) {
      failure = error instanceof Error ? error : new Error(String(error));
      for (const tracked of pending.values()) {
        tracked.reject(failure);
      }
      pending.clear();
      throw failure;
    }
  }

  function receive(bytes) {
    if (bytes.byteLength !== PAYLOAD_BYTES) {
      throw new Error(`delivery payload has ${bytes.byteLength} bytes`);
    }
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const runId = view.getUint32(0, true);
    const index = view.getUint32(4, true);
    const tracked = pending.get(runId);
    if (tracked === undefined) {
      throw new Error(`delivery arrived for unknown run ${runId}`);
    }
    if (index >= tracked.count || tracked.seen[index] !== 0) {
      throw new Error(`delivery run ${runId} has invalid index ${index}`);
    }
    const expected = payload(index);
    new DataView(expected.buffer).setUint32(0, runId, true);
    for (let offset = 0; offset < bytes.byteLength; offset += 1) {
      if (bytes[offset] !== expected[offset]) {
        throw new Error(`delivery run ${runId} index ${index} differs at ${offset}`);
      }
    }
    tracked.seen[index] = 1;
    tracked.received += 1;
    tracked.firstAt ??= performance.now();
    if (tracked.received === tracked.count) {
      pending.delete(runId);
      tracked.resolve({
        firstAt: tracked.firstAt,
        completedAt: performance.now(),
      });
    }
  }
}

async function loadSession() {
  const response = await fetch("/browser-full-engine-session");
  if (!response.ok) {
    throw new Error(`full-engine session returned HTTP ${response.status}`);
  }
  return response.json();
}

async function progress(stage) {
  await fetch(`/browser-full-engine-progress?stage=${encodeURIComponent(stage)}`);
}

function relayPeerUrl(webSocketUrl, peer) {
  const url = new URL(webSocketUrl);
  url.searchParams.set("peer", peer);
  return url.href;
}

async function startRelayMeasurement(peer, expected) {
  const url = new URL("/browser-full-engine-relay-measure-start", location.href);
  url.searchParams.set("peer", peer);
  url.searchParams.set("expected", String(expected));
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`relay measurement start returned HTTP ${response.status}`);
  }
  return response.json();
}

async function waitForRelayMeasurement(id) {
  const deadline = performance.now() + 10_000;
  while (true) {
    const url = new URL("/browser-full-engine-relay-measure", location.href);
    url.searchParams.set("id", String(id));
    const response = await fetch(url);
    if (!response.ok) {
      throw new Error(`relay measurement returned HTTP ${response.status}`);
    }
    const measurement = await response.json();
    if (measurement.completeMillis !== undefined) {
      return measurement;
    }
    if (performance.now() >= deadline) {
      throw new Error(
        `relay measured ${measurement.count}/${measurement.expected} frames`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
}

function requireTag(value, tag, operation) {
  if (value.tag !== tag) {
    throw new Error(`${operation} failed: ${JSON.stringify(value)}`);
  }
  return value;
}

function requireSettlement(value, outcome, operation) {
  if (value.tag !== "Succeeded" || value.data.tag !== outcome) {
    throw new Error(`${operation} failed: ${JSON.stringify(value)}`);
  }
  return value;
}

async function measure(operation) {
  const started = performance.now();
  const value = await operation();
  return { millis: performance.now() - started, value };
}

function median(values) {
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.floor(ordered.length / 2)];
}

async function waitFor(predicate) {
  const deadline = performance.now() + 5_000;
  while (!predicate()) {
    if (performance.now() >= deadline) {
      throw new Error("full-engine projections did not converge");
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

async function reportFailure(error) {
  const detail = error instanceof Error ? error.stack : String(error);
  document.getElementById("result").textContent = detail;
  await fetch("/browser-full-engine-result", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ error: detail }),
  });
}
