import {
  Prns,
  Tag,
  prnsView,
} from "/prns-js/dist/browser/index.js";

const REPETITIONS = 1;
const RESOURCE_REPETITIONS = 3;
const PAYLOAD_BYTES = 64;
const RESOURCE_SIZES = [1, 2, 4].map((mebibytes) => mebibytes * 1_024 * 1_024);
const RESOURCE_ACCEPTANCE_BYTES = 8 * 1_024 * 1_024;
const RESOURCE_TIMEOUT_MILLIS = 10_000;
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
  await progress("platform-ceilings");
  const platform = await platformCeilings(session.webSocketUrl);
  await progress("target-portable-wasm");
  const portableTarget = await prepareTarget(session.webSocketUrl, "PortableWasm");
  await progress("portable-wasm");
  const portableWasm = await prepare(
    "PortableWasm",
    session.webSocketUrl,
    portableTarget,
    Tag("PortableWasm"),
  );
  await progress("target-web-crypto");
  const webCryptoTarget = await prepareTarget(session.webSocketUrl, "WebCrypto");
  await progress("web-crypto");
  const webCrypto = await prepare(
    "WebCrypto",
    session.webSocketUrl,
    webCryptoTarget,
    Tag("WebCrypto"),
  );
  const configurations = [portableWasm, webCrypto];
  const targets = [portableTarget, webCryptoTarget];
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
  const resourceMeasurements = new Map(
    RESOURCE_SIZES.map((size) => [
      size,
      new Map(configurations.map((configuration) => [configuration.execution, []])),
    ]),
  );
  try {
    for (const configuration of configurations) {
      await progress(`warmup-${configuration.execution}`);
      await commandRun(configuration, workloads[1], payloads);
      await resourceRun(configuration, 64 * 1_024);
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
          ),
        );
      }
    }
    for (let repetition = 0; repetition < RESOURCE_REPETITIONS; repetition += 1) {
      const order = repetition % 2 === 0
        ? configurations
        : [...configurations].reverse();
      for (const size of RESOURCE_SIZES) {
        for (const configuration of order) {
          await progress(`${repetition}-resource-${size}-${configuration.execution}`);
          resourceMeasurements.get(size).get(configuration.execution).push(
            await resourceRun(configuration, size),
          );
        }
      }
    }
    const result = {
      userAgent: navigator.userAgent,
      wasmArtifact: session.wasmArtifact,
      platform,
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
      resources: RESOURCE_SIZES.map((size) => ({
        bytes: size,
        results: configurations.map((configuration) => summarizeResource(
          configuration.execution,
          size,
          resourceMeasurements.get(size).get(configuration.execution),
        )),
      })),
    };
    document.getElementById("result").textContent = JSON.stringify(result, null, 2);
    await Promise.all(configurations.map((configuration) => configuration.stop()));
    await Promise.all(targets.map((target) => target.stop()));
    await fetch("/browser-full-engine-result", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(result),
    });
  } catch (error) {
    await Promise.allSettled([
      ...configurations.map((configuration) => configuration.stop()),
      ...targets.map((target) => target.stop()),
    ]);
    throw error;
  }
}

async function prepare(execution, webSocketUrl, target, resourceCrypto) {
  const started = performance.now();
  const created = await Prns.create({
    execution: "DedicatedWorker",
    networkExecution: "NetworkWorker",
    resourceCrypto,
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
    prns.interfaces.webSocket.connect(relayPeerUrl(webSocketUrl, execution, execution))
  );
  requireTag(connected.value, "Connected", `${execution} WebSocket connection`);
  const discovered = await measure(() => prns.requestPath(target.destination));
  if (discovered.value.tag !== "Succeeded") {
    throw new Error(`${execution} path debug: ${JSON.stringify({
      lifecycle: prns.lifecycle,
      settlement: discovered.value,
      hostSnapshot: await prns.hostSnapshot(),
      diagnostics: diagnostics.latest(),
    }, (_key, value) => typeof value === "bigint" ? value.toString() : value)}`);
  }
  requireSettlement(discovered.value, "PathDiscovered", `${execution} path request`);
  const established = await measure(() => prns.establishLink(target.destination));
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
    deliveries: target.deliveries,
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

async function prepareTarget(webSocketUrl, lane) {
  const created = await Prns.create({
    execution: "DedicatedWorker",
    networkExecution: "EngineWorker",
    resourceCrypto: Tag(lane),
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
  requireSettlement(
    await prns.setDestinationResourceStrategy(
      registered.data,
      Tag("Accept", {
        maximumUncompressedBytes: RESOURCE_ACCEPTANCE_BYTES,
        acceptCompressed: false,
      }),
    ),
    "ResourceStrategySet",
    "target resource strategy",
  );
  const claimed = prns.claimEvents();
  requireTag(claimed, "Claimed", "target application event claim");
  const deliveries = deliveryTracker(claimed.data);
  const connected = await prns.interfaces.webSocket.connect(
    relayPeerUrl(webSocketUrl, "Target", lane),
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

async function commandRun(configuration, workload, payloads) {
  const observer = taskDelayObserver();
  const runId = nextRunId;
  nextRunId += 1;
  const runPayloads = payloadsForRun(payloads, workload.commands, runId);
  const delivered = configuration.deliveries.expect(runId, workload.commands);
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

async function contentionRun(configuration, workload, payloads) {
  const runId = nextRunId;
  nextRunId += 1;
  const runPayloads = payloadsForRun(payloads, workload.packets, runId);
  const delivered = configuration.deliveries.expect(runId, workload.packets);
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

async function resourceRun(configuration, size) {
  const runId = nextRunId;
  nextRunId += 1;
  const payload = resourcePayload(size, runId);
  const blob = new Blob([payload]);
  const metadata = new Uint8Array(4);
  new DataView(metadata.buffer).setUint32(0, runId, true);
  const delivered = configuration.deliveries.expectResource(runId, payload);
  const relayMeasurement = await startRelaySpan(configuration.execution);
  const observer = taskDelayObserver();
  const started = performance.now();
  const submissionStarted = performance.now();
  const pending = configuration.prns.sendResourceBlob(configuration.linkId, blob, {
    compression: Tag("Never"),
    packedMetadata: metadata,
  });
  const submissionMillis = performance.now() - submissionStarted;
  let settlementTiming;
  try {
    settlementTiming = await within(
      pending.then((value) => ({ value, completedAt: performance.now() })),
      RESOURCE_TIMEOUT_MILLIS,
      `${configuration.execution} resource settlement`,
    );
  } catch (error) {
    const relay = await stopRelaySpan(relayMeasurement.id);
    const snapshot = await configuration.prns.snapshot();
    const host = await configuration.prns.hostSnapshot();
    throw new Error(JSON.stringify({
      cause: error instanceof Error ? error.message : String(error),
      relay,
      snapshot,
      host,
    }, (_key, value) => typeof value === "bigint" ? value.toString() : value));
  }
  const settlement = settlementTiming.value;
  const settlementMillis = settlementTiming.completedAt - started;
  requireSettlement(settlement, "ResourceSent", `${configuration.execution} resource`);
  const delivery = await within(
    delivered,
    RESOURCE_TIMEOUT_MILLIS,
    `${configuration.execution} resource delivery`,
  );
  const relay = await stopRelaySpan(relayMeasurement.id);
  return {
    submissionMillis,
    settlementMillis,
    availableMillis: delivery.availableAt - started,
    assembledMillis: delivery.assembledAt - started,
    deliveryMillis: delivery.completedAt - started,
    verificationMillis: delivery.completedAt - delivery.assembledAt,
    segmentArrivalMillis: delivery.segmentArrivals?.map((arrival) => arrival - started),
    segmentBytes: delivery.segmentBytes,
    relayFrames: relay.count,
    relayBytes: relay.bytes,
    relayFirstMillis: relay.firstMillis,
    relayLastMillis: relay.lastMillis,
    relaySpanMillis: relay.lastMillis - relay.firstMillis,
    maximumEventLoopGapMillis: await observer.stop(),
  };
}

async function within(promise, milliseconds, label) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error(`${label} timed out after ${milliseconds}ms`)),
          milliseconds,
        );
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
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

function summarizeResource(execution, bytes, samples) {
  const settlementMillis = median(samples.map((sample) => sample.settlementMillis));
  const deliveryMillis = median(samples.map((sample) => sample.deliveryMillis));
  return {
    execution,
    submissionMillis: median(samples.map((sample) => sample.submissionMillis)),
    settlementMillis,
    availableMillis: median(samples.map((sample) => sample.availableMillis)),
    assembledMillis: median(samples.map((sample) => sample.assembledMillis)),
    deliveryMillis,
    verificationMillis: median(samples.map((sample) => sample.verificationMillis)),
    segmentArrivalMillis: medianColumns(
      samples.map((sample) => sample.segmentArrivalMillis ?? []),
    ),
    segmentBytes: samples[0].segmentBytes ?? [],
    relayFrames: median(samples.map((sample) => sample.relayFrames)),
    relayBytes: median(samples.map((sample) => sample.relayBytes)),
    relayFirstMillis: median(samples.map((sample) => sample.relayFirstMillis)),
    relayLastMillis: median(samples.map((sample) => sample.relayLastMillis)),
    relaySpanMillis: median(samples.map((sample) => sample.relaySpanMillis)),
    settlementMebibytesPerSecond: bytes / (settlementMillis / 1_000) / (1_024 * 1_024),
    deliveredMebibytesPerSecond: bytes / (deliveryMillis / 1_000) / (1_024 * 1_024),
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

function resourcePayload(size, runId) {
  return Uint8Array.from(
    { length: size },
    (_, index) => (index * 31 + runId * 17) & 0xff,
  );
}

function deliveryTracker(events) {
  const pending = new Map();
  const resourceSegments = new Map();
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
    expectResource(runId, expected) {
      if (failure !== undefined) {
        return Promise.reject(failure);
      }
      if (pending.has(runId)) {
        return Promise.reject(new Error(`duplicate resource run ${runId}`));
      }
      return new Promise((resolve, reject) => {
        pending.set(runId, { expected, resolve, reject });
      });
    },
  };

  async function consume() {
    try {
      for await (const event of events) {
        if (event.tag === "LinkDelivery") {
          receive(event.data.plaintext);
        }
        if (event.tag === "ResourceAvailable") {
          await receiveResource(event.data);
        }
        if (event.tag === "ResourceSegment") {
          receiveResourceSegment(event.data);
        }
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

  async function receiveResource(data) {
    if (data.metadata?.byteLength !== 4) {
      throw new Error("resource delivery omitted its run metadata");
    }
    const runId = new DataView(
      data.metadata.buffer,
      data.metadata.byteOffset,
      data.metadata.byteLength,
    ).getUint32(0, true);
    const tracked = pending.get(runId);
    if (tracked === undefined || tracked.expected === undefined) {
      throw new Error(`resource arrived for unknown run ${runId}`);
    }
    if (data.resource.totalBytes !== BigInt(tracked.expected.byteLength)) {
      throw new Error(`resource run ${runId} reported ${data.resource.totalBytes} bytes`);
    }
    const claimed = data.resource.claim();
    if (claimed.tag !== "Claimed") {
      throw new Error(`resource run ${runId} could not be claimed`);
    }
    const availableAt = performance.now();
    let offset = 0;
    for await (const chunk of claimed.data) {
      for (let index = 0; index < chunk.byteLength; index += 1) {
        if (chunk[index] !== tracked.expected[offset + index]) {
          throw new Error(`resource run ${runId} differs at ${offset + index}`);
        }
      }
      offset += chunk.byteLength;
    }
    if (offset !== tracked.expected.byteLength) {
      throw new Error(`resource run ${runId} delivered ${offset} bytes`);
    }
    const assembledAt = performance.now();
    pending.delete(runId);
    tracked.resolve({ availableAt, assembledAt, completedAt: performance.now() });
  }

  function receiveResourceSegment(data) {
    const key = hexadecimal(data.originalHash);
    const state = resourceSegments.get(key) ?? {
      totalSegments: data.totalSegments,
      segments: new Map(),
      arrivals: new Map(),
      availableAt: performance.now(),
      runId: undefined,
    };
    if (state.totalSegments !== data.totalSegments) {
      throw new Error(`resource ${key} changed its segment count`);
    }
    if (state.segments.has(data.segmentIndex)) {
      throw new Error(`resource ${key} repeated segment ${data.segmentIndex}`);
    }
    state.segments.set(data.segmentIndex, data.data);
    state.arrivals.set(data.segmentIndex, performance.now());
    if (data.metadata !== undefined) {
      if (data.metadata.byteLength !== 4) {
        throw new Error(`resource ${key} has invalid run metadata`);
      }
      const runId = new DataView(
        data.metadata.buffer,
        data.metadata.byteOffset,
        data.metadata.byteLength,
      ).getUint32(0, true);
      if (state.runId !== undefined && state.runId !== runId) {
        throw new Error(`resource ${key} changed its run metadata`);
      }
      state.runId = runId;
    }
    resourceSegments.set(key, state);
    if (state.runId === undefined || state.segments.size !== state.totalSegments) {
      return;
    }
    const tracked = pending.get(state.runId);
    if (tracked === undefined || tracked.expected === undefined) {
      throw new Error(`resource arrived for unknown run ${state.runId}`);
    }
    const assembledAt = performance.now();
    let offset = 0;
    for (let segmentIndex = 1; segmentIndex <= state.totalSegments; segmentIndex += 1) {
      const chunk = state.segments.get(segmentIndex);
      if (chunk === undefined) {
        throw new Error(`resource ${key} omitted segment ${segmentIndex}`);
      }
      for (let index = 0; index < chunk.byteLength; index += 1) {
        if (chunk[index] !== tracked.expected[offset + index]) {
          throw new Error(`resource run ${state.runId} differs at ${offset + index}`);
        }
      }
      offset += chunk.byteLength;
    }
    if (offset !== tracked.expected.byteLength) {
      throw new Error(`resource run ${state.runId} delivered ${offset} bytes`);
    }
    resourceSegments.delete(key);
    pending.delete(state.runId);
    tracked.resolve({
      availableAt: state.availableAt,
      assembledAt,
      completedAt: performance.now(),
      segmentArrivals: Array.from(
        { length: state.totalSegments },
        (_, index) => state.arrivals.get(index + 1),
      ),
      segmentBytes: Array.from(
        { length: state.totalSegments },
        (_, index) => state.segments.get(index + 1).byteLength,
      ),
    });
  }
}

async function platformCeilings(webSocketUrl) {
  const worker = await workerTransferCeiling();
  const webSocket = await webSocketRelayCeiling(webSocketUrl);
  const wasmCrypto = await wasmCryptoCeiling();
  const webCrypto = await webCryptoCeiling();
  return { worker, webSocket, wasmCrypto, webCrypto };
}

async function webCryptoCeiling() {
  const aesKey = await crypto.subtle.importKey(
    "raw",
    new Uint8Array(32).fill(0x5a),
    "AES-CBC",
    false,
    ["encrypt", "decrypt"],
  );
  const hmacKey = await crypto.subtle.importKey(
    "raw",
    new Uint8Array(32).fill(0x5a),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign", "verify"],
  );
  const iv = new Uint8Array(16).fill(0x3c);
  const wasm = await import("/prns-js/wasm/prns_wasm.js");
  await wasm.default();
  const vectorPayload = new Uint8Array(4_093).fill(0xa5);
  const vectorCipher = new Uint8Array(await crypto.subtle.encrypt(
    { name: "AES-CBC", iv },
    aesKey,
    vectorPayload,
  ));
  const vectorSigned = new Uint8Array(iv.byteLength + vectorCipher.byteLength);
  vectorSigned.set(iv);
  vectorSigned.set(vectorCipher, iv.byteLength);
  const vectorTag = new Uint8Array(await crypto.subtle.sign(
    "HMAC",
    hmacKey,
    vectorSigned,
  ));
  const webCryptoToken = new Uint8Array(vectorSigned.byteLength + vectorTag.byteLength);
  webCryptoToken.set(vectorSigned);
  webCryptoToken.set(vectorTag, vectorSigned.byteLength);
  const wasmToken = wasm.profileTokenVector(vectorPayload.byteLength);
  verifyResourcePayload(webCryptoToken, wasmToken, "Web Crypto token vector");
  const results = [];
  for (const size of RESOURCE_SIZES) {
    const iterations = Math.max(2, Math.ceil((8 * 1_024 * 1_024) / size));
    const payload = new Uint8Array(size).fill(0xa5);
    const cipher = await crypto.subtle.encrypt(
      { name: "AES-CBC", iv },
      aesKey,
      payload,
    );
    const tag = await crypto.subtle.sign("HMAC", hmacKey, cipher);
    results.push({
      bytes: size,
      iterations,
      seal: await measureWebCryptoThroughput(async () => {
        const encrypted = await crypto.subtle.encrypt(
          { name: "AES-CBC", iv },
          aesKey,
          payload,
        );
        await crypto.subtle.sign("HMAC", hmacKey, encrypted);
      }, size * iterations, iterations),
      open: await measureWebCryptoThroughput(async () => {
        const authentic = await crypto.subtle.verify("HMAC", hmacKey, tag, cipher);
        if (!authentic) {
          throw new Error("Web Crypto HMAC verification failed");
        }
        await crypto.subtle.decrypt({ name: "AES-CBC", iv }, aesKey, cipher);
      }, size * iterations, iterations),
      sha256: await measureWebCryptoThroughput(
        () => crypto.subtle.digest("SHA-256", payload),
        size * iterations,
        iterations,
      ),
    });
  }
  return { tokenMatchesWasm: true, results };
}

async function measureWebCryptoThroughput(operation, bytes, iterations) {
  const samples = [];
  for (let repetition = 0; repetition < 3; repetition += 1) {
    const started = performance.now();
    for (let iteration = 0; iteration < iterations; iteration += 1) {
      await operation();
    }
    samples.push(performance.now() - started);
  }
  const elapsedMillis = median(samples);
  return {
    elapsedMillis,
    mebibytesPerSecond: bytes / (elapsedMillis / 1_000) / (1_024 * 1_024),
  };
}

async function wasmCryptoCeiling() {
  const wasm = await import("/prns-js/wasm/prns_wasm.js");
  await wasm.default();
  wasm.profileTokenSeal(64 * 1_024, 1);
  wasm.profileTokenOpen(64 * 1_024, 1);
  wasm.profileSha256(64 * 1_024, 1);
  return RESOURCE_SIZES.map((size) => {
    const iterations = Math.max(2, Math.ceil((8 * 1_024 * 1_024) / size));
    return {
      bytes: size,
      iterations,
      seal: measureWasmThroughput(
        () => wasm.profileTokenSeal(size, iterations),
        size * iterations,
      ),
      open: measureWasmThroughput(
        () => wasm.profileTokenOpen(size, iterations),
        size * iterations,
      ),
      sha256: measureWasmThroughput(
        () => wasm.profileSha256(size, iterations),
        size * iterations,
      ),
    };
  });
}

function measureWasmThroughput(operation, bytes) {
  const samples = [];
  for (let repetition = 0; repetition < 3; repetition += 1) {
    const started = performance.now();
    operation();
    samples.push(performance.now() - started);
  }
  const elapsedMillis = median(samples);
  return {
    elapsedMillis,
    mebibytesPerSecond: bytes / (elapsedMillis / 1_000) / (1_024 * 1_024),
  };
}

async function workerTransferCeiling() {
  const workerSource = `self.onmessage = ({ data }) => self.postMessage(data, [data])`;
  const worker = new Worker(URL.createObjectURL(new Blob([workerSource])));
  const roundTrip = (buffer) => new Promise((resolve, reject) => {
    worker.onmessage = ({ data }) => resolve(data);
    worker.onerror = ({ message }) => reject(new Error(message));
    worker.postMessage(buffer, [buffer]);
  });
  await roundTrip(new ArrayBuffer(64 * 1_024));
  const results = [];
  for (const size of RESOURCE_SIZES) {
    const samples = [];
    for (let repetition = 0; repetition < 5; repetition += 1) {
      const buffer = new ArrayBuffer(size);
      const started = performance.now();
      const returned = await roundTrip(buffer);
      const roundTripMillis = performance.now() - started;
      if (returned.byteLength !== size) {
        throw new Error(`worker transfer returned ${returned.byteLength}/${size} bytes`);
      }
      samples.push(roundTripMillis);
    }
    const roundTripMillis = median(samples);
    results.push({
      bytes: size,
      roundTripMillis,
      bidirectionalMebibytesPerSecond:
        size * 2 / (roundTripMillis / 1_000) / (1_024 * 1_024),
    });
  }
  worker.terminate();
  return results;
}

async function webSocketRelayCeiling(webSocketUrl) {
  const lane = `Bare-${nextRunId}`;
  nextRunId += 1;
  const sender = await openWebSocket(relayPeerUrl(webSocketUrl, "BareSender", lane));
  const receiver = await openWebSocket(relayPeerUrl(webSocketUrl, "BareTarget", lane));
  const transfer = (payload) => new Promise((resolve, reject) => {
    receiver.onmessage = ({ data }) => resolve(new Uint8Array(data));
    receiver.onerror = () => reject(new Error("bare WebSocket receiver failed"));
    sender.send(payload);
  });
  await transfer(resourcePayload(64 * 1_024, nextRunId));
  const results = [];
  for (const size of RESOURCE_SIZES) {
    const deliverySamples = [];
    const verificationSamples = [];
    for (let repetition = 0; repetition < 5; repetition += 1) {
      const runId = nextRunId;
      nextRunId += 1;
      const payload = resourcePayload(size, runId);
      const started = performance.now();
      const received = await transfer(payload);
      const deliveredAt = performance.now();
      verifyResourcePayload(received, payload, runId);
      deliverySamples.push(deliveredAt - started);
      verificationSamples.push(performance.now() - deliveredAt);
    }
    const deliveryMillis = median(deliverySamples);
    results.push({
      bytes: size,
      deliveryMillis,
      verificationMillis: median(verificationSamples),
      deliveredMebibytesPerSecond:
        size / (deliveryMillis / 1_000) / (1_024 * 1_024),
    });
  }
  sender.close();
  receiver.close();
  return results;
}

function openWebSocket(url) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    socket.onopen = () => resolve(socket);
    socket.onerror = () => reject(new Error(`WebSocket failed for ${url}`));
  });
}

function verifyResourcePayload(received, expected, runId) {
  if (received.byteLength !== expected.byteLength) {
    throw new Error(`resource run ${runId} delivered ${received.byteLength} bytes`);
  }
  for (let index = 0; index < received.byteLength; index += 1) {
    if (received[index] !== expected[index]) {
      throw new Error(`resource run ${runId} differs at ${index}`);
    }
  }
}

function hexadecimal(bytes) {
  let value = "";
  for (const byte of bytes) {
    value += byte.toString(16).padStart(2, "0");
  }
  return value;
}

async function loadSession() {
  const response = await fetch("/browser-full-engine-session");
  if (!response.ok) {
    throw new Error(`full-engine session returned HTTP ${response.status}`);
  }
  return response.json();
}

async function progress(stage) {
  performance.mark(`prns-browser-full-engine:${stage}`);
  await fetch(`/browser-full-engine-progress?stage=${encodeURIComponent(stage)}`);
}

function relayPeerUrl(webSocketUrl, peer, lane) {
  const url = new URL(webSocketUrl);
  url.searchParams.set("peer", peer);
  url.searchParams.set("lane", lane);
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

async function startRelaySpan(peer) {
  const url = new URL("/browser-full-engine-relay-span-start", location.href);
  url.searchParams.set("peer", peer);
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`relay span start returned HTTP ${response.status}`);
  }
  return response.json();
}

async function stopRelaySpan(id) {
  const url = new URL("/browser-full-engine-relay-span-stop", location.href);
  url.searchParams.set("id", String(id));
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`relay span stop returned HTTP ${response.status}`);
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

function medianColumns(rows) {
  const columns = Math.max(0, ...rows.map((row) => row.length));
  return Array.from(
    { length: columns },
    (_, index) => median(rows.map((row) => row[index])),
  );
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
