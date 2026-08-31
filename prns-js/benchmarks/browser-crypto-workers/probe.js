import { Tag, match } from "../../dist/casework.js";

let port;
let activeProbe;
let nextSequence = 1;
const finishedProbes = new Set();

self.addEventListener("message", ({ data }) => {
  match(data, {
    Initialize: ({ port: initializedPort }) => {
      port = initializedPort;
      port.addEventListener("message", ({ data: message }) => {
        match(message, {
          BeginProbe: ({ id }) => beginProbe(id),
          EndProbe: ({ id }) => endProbe(id),
          Pong: ({ id, sequence }) => receivePong(id, sequence),
        });
      });
      port.start();
    },
  });
});

function beginProbe(id) {
  if (activeProbe !== undefined) {
    throw new Error("coordinator latency probe overlapped another measurement");
  }
  activeProbe = {
    id,
    samples: [],
    pending: undefined,
    interval: setInterval(sendPing, 1),
  };
  sendPing();
  port.postMessage(Tag("ProbeStarted", { id }));
}

function sendPing() {
  if (activeProbe === undefined || activeProbe.pending !== undefined) {
    return;
  }
  const sequence = nextSequence;
  nextSequence += 1;
  activeProbe.pending = {
    sequence,
    sentAt: performance.now(),
  };
  port.postMessage(Tag("Ping", {
    id: activeProbe.id,
    sequence,
  }));
}

function receivePong(id, sequence) {
  if (finishedProbes.has(id)) {
    return;
  }
  if (
    activeProbe === undefined ||
    activeProbe.id !== id ||
    activeProbe.pending?.sequence !== sequence
  ) {
    throw new Error("coordinator latency probe received an unexpected pong");
  }
  activeProbe.samples.push(performance.now() - activeProbe.pending.sentAt);
  activeProbe.pending = undefined;
}

function endProbe(id) {
  if (activeProbe === undefined || activeProbe.id !== id) {
    throw new Error("coordinator latency probe stopped an unknown measurement");
  }
  clearInterval(activeProbe.interval);
  if (activeProbe.pending !== undefined) {
    activeProbe.samples.push(performance.now() - activeProbe.pending.sentAt);
  }
  const samples = [...activeProbe.samples].sort((left, right) => left - right);
  const result = {
    id,
    samples: samples.length,
    medianMillis: percentile(samples, 0.5),
    p95Millis: percentile(samples, 0.95),
    maximumMillis: samples.at(-1) ?? 0,
  };
  activeProbe = undefined;
  finishedProbes.add(id);
  if (finishedProbes.size > 16) {
    const oldest = finishedProbes.values().next().value;
    if (oldest !== undefined) {
      finishedProbes.delete(oldest);
    }
  }
  port.postMessage(Tag("ProbeResult", result));
}

function percentile(values, fraction) {
  if (values.length === 0) {
    return 0;
  }
  return values[Math.min(values.length - 1, Math.floor(values.length * fraction))];
}
