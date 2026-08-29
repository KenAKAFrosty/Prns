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

const requestDecoder = new WireBatchDecoder();
const responseEncoder = new WireBatchEncoder({ minimumItems: 1 });
const settlementSender = new BatchedPortSender({
  port: self,
  wrap: (batch) => Tag("SettlementFrame", { batch }),
  maximumItems: MAXIMUM_WIRE_BATCH_ITEMS,
  maximumBytes: 1024 * 1024,
  scheduleTask: messageTaskScheduler(),
  failed: fail,
});
const commandReceiver = new BatchedPortReceiver((command) => {
  void settleCommand(command);
});

self.addEventListener("message", ({ data }) => {
  match(data, {
    CloneRows: ({ id, rows }) => {
      self.postMessage(Tag("CloneRows", { id, rows }));
    },
    PackedRows: ({ id, batch }) => {
      const rows = requestDecoder.decode(batch)[0];
      const encoded = responseEncoder.encode([rows]);
      self.postMessage(
        Tag("PackedRows", { id, batch: encoded.message }),
        [...encoded.transfer],
      );
    },
    CloneCommand: async ({ id, value }) => {
      const outcome = await commandOutcome(value);
      self.postMessage(Tag("CloneSettlement", { id, outcome }));
    },
    CommandFrame: ({ batch }) => commandReceiver.receive(batch),
  });
});

async function settleCommand(command) {
  settlementSender.send({
    id: command.id,
    outcome: await commandOutcome(command.value),
  });
}

async function commandOutcome(value) {
  return match(value, {
    Advance: ({ index, delta }) => ({ checksum: index * 31 + delta }),
    Blend: ({ index, left, right, ratio }) => ({
      checksum: index * 31 + left * ratio + right * (1 - ratio),
    }),
    Digest: ({ index, payload }) => ({
      checksum: payload.reduce((total, byte) => (total + byte) >>> 0, index),
    }),
    Describe: ({ index, active, name }) => ({
      checksum: index * 31 + name.length + (active ? 1 : 0),
    }),
    Delay: async ({ index, delayMillis }) => {
      await new Promise((resolve) => setTimeout(resolve, delayMillis));
      return { checksum: index };
    },
  });
}

function fail(error) {
  self.postMessage(Tag("Failed", { detail: String(error?.stack ?? error) }));
}
