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
import {
  workerInvocationCodec,
  workerSettlementCodec,
} from "../../dist/browser/worker_codecs.js";
import {
  DESTINATION_HASH_LENGTH,
  LINK_ID_LENGTH,
  REQUEST_PATH_HASH_LENGTH,
} from "../../dist/contract.js";

const worker = new Worker("./worker.js", { type: "module" });
const requestEncoder = new WireBatchEncoder();
const responseDecoder = new WireBatchDecoder();
const pending = new Map();
const clonePending = new Map();
const numericTransferPending = new Map();
const commandPending = new Map();
let nextId = 1;

const clonedBatchCommandSender = commandSender(
  "ClonedCommandFrame",
  {},
);
const codecBatchCommandSender = commandSender(
  "CodecCommandFrame",
  { codec: workerInvocationCodec },
);

function commandSender(frameTag, options) {
  return new BatchedPortSender({
    port: worker,
    wrap: (batch) => Tag(frameTag, { batch }),
    maximumItems: MAXIMUM_WIRE_BATCH_ITEMS,
    maximumQueuedItems: MAXIMUM_WIRE_BATCH_ITEMS,
    maximumBytes: 1024 * 1024,
    measureBytes: () => 256,
    scheduleTask: messageTaskScheduler(),
    failed: fail,
    ...options,
  });
}

const settlementReceiver = new BatchedPortReceiver(
  (settlement) => {
    const resolve = commandPending.get(settlement.id);
    if (resolve !== undefined) {
      commandPending.delete(settlement.id);
      resolve(settlement.outcome);
    }
  },
  [workerSettlementCodec],
);

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
    TransferredNumbers: ({ id, buffer }) => {
      const resolve = numericTransferPending.get(id);
      numericTransferPending.delete(id);
      resolve(new Float64Array(buffer));
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

function transferNumbers(values) {
  const id = nextId++;
  const buffer = values.buffer;
  return new Promise((resolve) => {
    numericTransferPending.set(id, resolve);
    worker.postMessage(Tag("TransferNumbers", { id, buffer }), [buffer]);
  });
}

function cloneCommand(command) {
  const id = nextId++;
  const invocation = { id, call: Tag("Execute", command) };
  return new Promise((resolve) => {
    commandPending.set(id, resolve);
    worker.postMessage(Tag("CloneCommand", { invocation }));
  });
}

function batchedCommand(sender, command) {
  const id = nextId++;
  return new Promise((resolve) => {
    commandPending.set(id, resolve);
    sender.send({ id, call: Tag("Execute", command) });
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
  const framedClone = [];
  for (let repetition = 0; repetition < 7; repetition += 1) {
    const order = repetition % 2 === 0
      ? [[clone, cloneRows], [framedClone, packedRows]]
      : [[framedClone, packedRows], [clone, cloneRows]];
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
    framedCloneMedianMs: median(framedClone),
  };
}

async function numericRound(count) {
  const value = Array.from({ length: count }, (_, index) => index + 0.25);
  let transferred = await transferNumbers(Float64Array.from(value));
  await cloneRows(value);
  await packedRows(value);
  const clone = [];
  const framedClone = [];
  const transfer = [];
  for (let repetition = 0; repetition < 7; repetition += 1) {
    const configurations = [
      [clone, async () => cloneRows(value)],
      [framedClone, async () => packedRows(value)],
      [transfer, async () => {
        transferred = await transferNumbers(transferred);
        return transferred;
      }],
    ];
    const order = configurations.map((_, index) =>
      configurations[(index + repetition) % configurations.length]
    );
    for (const [samples, operation] of order) {
      const measured = await measure(operation);
      if (
        measured.value.length !== count ||
        measured.value[0] !== 0.25 ||
        measured.value[count - 1] !== count - 0.75
      ) {
        throw new Error("numeric benchmark returned the wrong values");
      }
      samples.push(measured.millis);
    }
  }
  return {
    values: count,
    plainCloneMedianMs: median(clone),
    framedCloneMedianMs: median(framedClone),
    transferredF64MedianMs: median(transfer),
    transferSpeedup: median(clone) / median(transfer),
  };
}

async function commandRound(count) {
  const warm = Array.from({ length: 32 }, (_, index) => commandValue(index));
  await Promise.all(warm.map((value) => batchedCommand(clonedBatchCommandSender, value)));
  await Promise.all(warm.map((value) => batchedCommand(codecBatchCommandSender, value)));
  const clone = [];
  const clonedBatch = [];
  const codecBatch = [];
  const values = Array.from({ length: count }, (_, index) => commandValue(index));
  const roundsPerSample = Math.max(10, Math.ceil((count < 10 ? 1_000 : 10_000) / count));
  for (let repetition = 0; repetition < 7; repetition += 1) {
    const configurations = [
      [clone, cloneCommand],
      [clonedBatch, (value) => batchedCommand(clonedBatchCommandSender, value)],
      [codecBatch, (value) => batchedCommand(codecBatchCommandSender, value)],
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
    codecBatchMedianMs: median(codecBatch),
    clonedBatchSpeedup: median(clone) / median(clonedBatch),
    codecSpeedup: median(clonedBatch) / median(codecBatch),
  };
}

function commandValue(index) {
  if (index % 4 === 0) {
    return Tag("Announce", { destination: fixedBytes(DESTINATION_HASH_LENGTH, index) });
  }
  if (index % 4 === 1) {
    return Tag("SendSinglePacket", {
      destination: fixedBytes(DESTINATION_HASH_LENGTH, index),
      payload: fixedBytes(32 + index % 97, index + 1),
    });
  }
  if (index % 4 === 2) {
    return Tag("Request", {
      linkId: fixedBytes(LINK_ID_LENGTH, index),
      pathHash: fixedBytes(REQUEST_PATH_HASH_LENGTH, index + 1),
      payload: fixedBytes(48 + index % 67, index + 2),
      timeout: Tag("Exact", { millis: 1_000 + index }),
      maximumResponseBytes: 65_536,
    });
  }
  return Tag("SendChannelMessage", {
    linkId: fixedBytes(LINK_ID_LENGTH, index),
    messageType: index % 0xffff,
    payload: fixedBytes(24 + index % 53, index + 3),
  });
}

function fixedBytes(length, seed) {
  return Uint8Array.from({ length }, (_, index) => (seed + index) & 0xff);
}

async function settlementIsolation() {
  const fast = Array.from({ length: 100 }, (_, index) =>
    batchedCommand(codecBatchCommandSender, commandValue(index))
  );
  const slow = batchedCommand(
    codecBatchCommandSender,
    Tag("RequestPath", { destination: fixedBytes(DESTINATION_HASH_LENGTH, 0xff) }),
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
    numericArrays: [
      await numericRound(1_000),
      await numericRound(10_000),
      await numericRound(100_000),
    ],
    rows: [
      await rowRound(1_000),
      await rowRound(10_000),
      await rowRound(100_000),
    ],
    commands: [
      await commandRound(1),
      await commandRound(4),
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
