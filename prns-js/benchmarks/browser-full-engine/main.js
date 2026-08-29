import {
  Prns,
  destinationHash,
  prnsView,
} from "/prns-js/dist/browser/index.js";

const REPETITIONS = 5;
const PAYLOAD_BYTES = 64;
const workloads = [
  { name: "sequential", commands: 200, grain: 1 },
  { name: "coalesced", commands: 4_000, grain: 100 },
  { name: "bounded", commands: 4_096, grain: 256 },
];

run().catch(reportFailure);

async function run() {
  const session = await loadSession();
  const destination = destinationHash(hexadecimalBytes(session.destinationHex));
  const dedicated = await prepare("DedicatedWorker", session.webSocketUrl, destination);
  const main = await prepare("MainThread", session.webSocketUrl, destination);
  const configurations = [dedicated, main];
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
  try {
    for (const configuration of configurations) {
      await commandRun(configuration, workloads[1], payloads);
    }
    for (let repetition = 0; repetition < REPETITIONS; repetition += 1) {
      const order = repetition % 2 === 0
        ? configurations
        : [...configurations].reverse();
      for (const workload of workloads) {
        for (const configuration of order) {
          measurements[workload.name].get(configuration.execution).push(
            await commandRun(configuration, workload, payloads),
          );
        }
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
      projectionDemand: await measureProjectionDemand(configurations, payloads),
    };
    document.getElementById("result").textContent = JSON.stringify(result, null, 2);
    await Promise.all(configurations.map((configuration) => configuration.stop()));
    await fetch("/browser-full-engine-result", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(result),
    });
  } catch (error) {
    await Promise.allSettled(configurations.map((configuration) => configuration.stop()));
    throw error;
  }
}

async function prepare(execution, webSocketUrl, destination) {
  const started = performance.now();
  const created = await Prns.create(execution === "MainThread"
    ? { execution: "MainThread" }
    : {
        execution: "DedicatedWorker",
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
  const connected = await measure(() => prns.interfaces.webSocket.connect(webSocketUrl));
  requireTag(connected.value, "Connected", `${execution} WebSocket connection`);
  const discovered = await measure(() => prns.requestPath(destination));
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

async function measureProjectionDemand(configurations, payloads) {
  const workload = { name: "projection-demand", commands: 1_000, grain: 100 };
  const results = [];
  for (const configuration of configurations) {
    const samples = { Unobserved: [], Observed: [] };
    for (let repetition = 0; repetition < REPETITIONS; repetition += 1) {
      const order = repetition % 2 === 0
        ? ["Unobserved", "Observed"]
        : ["Observed", "Unobserved"];
      for (const observation of order) {
        configuration.setProjectionObservation(observation === "Observed");
        samples[observation].push(
          await commandRun(configuration, workload, payloads),
        );
      }
    }
    configuration.setProjectionObservation(false);
    results.push({
      execution: configuration.execution,
      commands: workload.commands,
      unobserved: summarize(
        configuration.execution,
        workload.commands,
        samples.Unobserved,
      ),
      observed: summarize(
        configuration.execution,
        workload.commands,
        samples.Observed,
      ),
    });
  }
  return results;
}

async function commandRun(configuration, workload, payloads) {
  const observer = taskDelayObserver();
  let submissionMillis = 0;
  const started = performance.now();
  for (let offset = 0; offset < workload.commands; offset += workload.grain) {
    const end = Math.min(offset + workload.grain, workload.commands);
    const submissionStarted = performance.now();
    const pending = [];
    for (let index = offset; index < end; index += 1) {
      pending.push(configuration.prns.sendLinkPacket(
        configuration.linkId,
        payloads[index],
      ));
    }
    submissionMillis += performance.now() - submissionStarted;
    const outcomes = await Promise.all(pending);
    for (const outcome of outcomes) {
      requireSettlement(outcome, "PacketDelivered", `${configuration.execution} link packet`);
    }
  }
  const elapsedMillis = performance.now() - started;
  return {
    elapsedMillis,
    submissionMillis,
    maximumEventLoopGapMillis: await observer.stop(),
  };
}

function summarize(execution, commands, samples) {
  const elapsedMillis = median(samples.map((sample) => sample.elapsedMillis));
  return {
    execution,
    elapsedMillis,
    submissionMillis: median(samples.map((sample) => sample.submissionMillis)),
    maximumEventLoopGapMillis: median(
      samples.map((sample) => sample.maximumEventLoopGapMillis),
    ),
    commandsPerSecond: commands / (elapsedMillis / 1_000),
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
  return Uint8Array.from(
    { length: PAYLOAD_BYTES },
    (_, byte) => (index * 31 + byte) & 0xff,
  );
}

async function loadSession() {
  const response = await fetch("/browser-full-engine-session");
  if (!response.ok) {
    throw new Error(`full-engine session returned HTTP ${response.status}`);
  }
  return response.json();
}

function hexadecimalBytes(value) {
  if (!/^[0-9a-f]{32}$/iu.test(value)) {
    throw new Error("full-engine destination must contain 32 hexadecimal digits");
  }
  return Uint8Array.from(
    { length: value.length / 2 },
    (_, index) => Number.parseInt(value.slice(index * 2, index * 2 + 2), 16),
  );
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
