import { Tag, match } from "../../dist/casework.js";
import {
  BatchedPortReceiver,
  BatchedPortSender,
  messageTaskScheduler,
} from "../../dist/worker_wire/batched_port.js";
import {
  MAXIMUM_WIRE_BATCH_ITEMS,
  WireBatchDecoder,
  WireBatchEncoder,
} from "../../dist/worker_wire/wire_batch.js";

const worker = new Worker("./worker.js", { type: "module" });
const requestEncoder = new WireBatchEncoder({ minimumItems: 1 });
const responseDecoder = new WireBatchDecoder();
const pending = new Map();
const clonePending = new Map();
const commandPending = new Map();
let nextId = 1;

const productionCommandSender = commandSender();
const clonedBatchCommandSender = commandSender({ minimumItems: Number.MAX_SAFE_INTEGER });
const packedBatchCommandSender = commandSender({ minimumItems: 1 });

function commandSender(packingPolicy) {
  return new BatchedPortSender({
    port: worker,
    wrap: (batch) => Tag("CommandFrame", { batch }),
    maximumItems: MAXIMUM_WIRE_BATCH_ITEMS,
    maximumBytes: 1024 * 1024,
    scheduleTask: messageTaskScheduler(),
    failed: fail,
    ...(packingPolicy === undefined ? {} : { packingPolicy }),
  });
}

const settlementReceiver = new BatchedPortReceiver((settlement) => {
  const resolve = commandPending.get(settlement.id);
  if (resolve !== undefined) {
    commandPending.delete(settlement.id);
    resolve(settlement.outcome);
  }
});

worker.addEventListener("message", ({ data }) => {
  match(data, {
    CloneRows: ({ id, rows }) => {
      const resolve = clonePending.get(id);
      clonePending.delete(id);
      resolve(rows);
    },
    PackedRows: ({ id, batch }) => {
      const resolve = pending.get(id);
      pending.delete(id);
      resolve(responseDecoder.decode(batch)[0]);
    },
    CloneSettlement: ({ id, outcome }) => {
      const resolve = commandPending.get(id);
      commandPending.delete(id);
      resolve(outcome);
    },
    SettlementFrame: ({ batch }) => settlementReceiver.receive(batch),
    Failed: ({ detail }) => fail(new Error(detail)),
  });
});

function cloneRows(rows) {
  const id = nextId++;
  return new Promise((resolve) => {
    clonePending.set(id, resolve);
    worker.postMessage(Tag("CloneRows", { id, rows }));
  });
}

function packedRows(rows) {
  const id = nextId++;
  const encoded = requestEncoder.encode([rows]);
  return new Promise((resolve) => {
    pending.set(id, resolve);
    worker.postMessage(
      Tag("PackedRows", { id, batch: encoded.message }),
      [...encoded.transfer],
    );
  });
}

function cloneCommand(value) {
  const id = nextId++;
  return new Promise((resolve) => {
    commandPending.set(id, resolve);
    worker.postMessage(Tag("CloneCommand", { id, value }));
  });
}

function batchedCommand(sender, value) {
  const id = nextId++;
  return new Promise((resolve) => {
    commandPending.set(id, resolve);
    sender.send({ id, value });
  });
}

function rows(count) {
  return Array.from({ length: count }, (_, index) => ({
    stuff: index % 3 === 0 ? "alpha" : "beta",
    name: `peer-${index}`,
    uuid: `00000000-0000-0000-0000-${String(index).padStart(12, "0")}`,
    nested: [
      { score: index + 0.25, recent: index % 2 === 0 },
      { score: index + 0.75, recent: index % 5 === 0 },
    ],
    last_seen_at: new Date(1_700_000_000_000 + index),
    enabled: index % 7 === 0,
    optional: index % 4 === 0 ? undefined : index,
    nullable: index % 6 === 0 ? null : index * 2,
    state: index % 2 === 0
      ? Tag("Ready", { peers: index })
      : Tag("Waiting", { sequence: index }),
  }));
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

async function rowRound(count) {
  const value = rows(count);
  await cloneRows(value);
  await packedRows(value);
  const clone = [];
  const packed = [];
  for (let repetition = 0; repetition < 7; repetition += 1) {
    const order = repetition % 2 === 0
      ? [[clone, cloneRows], [packed, packedRows]]
      : [[packed, packedRows], [clone, cloneRows]];
    for (const [samples, operation] of order) {
      const measured = await measure(() => operation(value));
      if (measured.value.length !== count) {
        throw new Error("row benchmark returned the wrong number of rows");
      }
      samples.push(measured.millis);
    }
  }
  return {
    rows: count,
    cloneMedianMs: median(clone),
    packedMedianMs: median(packed),
    speedup: median(clone) / median(packed),
  };
}

async function commandRound(count) {
  await Promise.all(Array.from({ length: 32 }, (_, index) =>
    batchedCommand(productionCommandSender, commandValue(index))
  ));
  const clone = [];
  const clonedBatch = [];
  const packedBatch = [];
  const values = Array.from({ length: count }, (_, index) => commandValue(index));
  const roundsPerSample = Math.max(10, Math.ceil(10_000 / count));
  for (let repetition = 0; repetition < 7; repetition += 1) {
    const configurations = [
      [clone, cloneCommand],
      [clonedBatch, (value) => batchedCommand(clonedBatchCommandSender, value)],
      [packedBatch, (value) => batchedCommand(packedBatchCommandSender, value)],
    ];
    const order = configurations.map((_, index) =>
      configurations[(index + repetition) % configurations.length]
    );
    for (const [samples, operation] of order) {
      const measured = await measure(async () => {
        for (let round = 0; round < roundsPerSample; round += 1) {
          await Promise.all(values.map(operation));
        }
      });
      samples.push(measured.millis);
    }
  }
  return {
    commands: count,
    roundsPerSample,
    cloneMedianMs: median(clone),
    clonedBatchMedianMs: median(clonedBatch),
    packedBatchMedianMs: median(packedBatch),
    clonedBatchSpeedup: median(clone) / median(clonedBatch),
    packedBatchSpeedup: median(clone) / median(packedBatch),
    packingSpeedup: median(clonedBatch) / median(packedBatch),
  };
}

function commandValue(index) {
  if (index % 4 === 0) {
    return Tag("Advance", { index, delta: index % 17 });
  }
  if (index % 4 === 1) {
    return Tag("Blend", {
      index,
      left: index + 0.25,
      right: index + 0.75,
      ratio: (index % 10) / 10,
    });
  }
  if (index % 4 === 2) {
    return Tag("Digest", {
      index,
      payload: Uint8Array.from({ length: 32 + index % 97 }, (_, offset) =>
        (index + offset) & 0xff
      ),
    });
  }
  return Tag("Describe", {
    index,
    active: index % 2 === 0,
    name: `peer-${index % 23}`,
  });
}

async function settlementIsolation() {
  const fast = Array.from({ length: 100 }, (_, index) =>
    batchedCommand(productionCommandSender, commandValue(index))
  );
  const slow = batchedCommand(
    productionCommandSender,
    Tag("Delay", { index: 100, delayMillis: 50 }),
  );
  const started = performance.now();
  await Promise.all(fast);
  const fastMillis = performance.now() - started;
  await slow;
  return { fastMillis, totalMillis: performance.now() - started };
}

async function run() {
  const result = {
    userAgent: navigator.userAgent,
    rows: [
      await rowRound(1_000),
      await rowRound(10_000),
      await rowRound(100_000),
    ],
    commands: [
      await commandRound(10),
      await commandRound(32),
      await commandRound(64),
      await commandRound(100),
      await commandRound(256),
      await commandRound(512),
      await commandRound(1_000),
      await commandRound(MAXIMUM_WIRE_BATCH_ITEMS),
    ],
    settlementIsolation: await settlementIsolation(),
  };
  document.querySelector("#result").textContent = JSON.stringify(result, null, 2);
  await fetch("/browser-worker-wire-result", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(result),
  });
}

function fail(error) {
  document.querySelector("#result").textContent = String(error?.stack ?? error);
  void fetch("/browser-worker-wire-result", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ error: String(error?.stack ?? error) }),
  });
}

run().catch(fail);
