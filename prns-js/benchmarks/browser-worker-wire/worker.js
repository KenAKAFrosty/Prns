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

const requestDecoder = new WireBatchDecoder();
const responseEncoder = new WireBatchEncoder({ minimumItems: 1 });
const clonedSettlementSender = settlementSender({
  packingPolicy: { minimumItems: Number.MAX_SAFE_INTEGER },
});
const packedSettlementSender = settlementSender({
  packingPolicy: { minimumItems: 1 },
});
const codecSettlementSender = settlementSender({ codec: workerSettlementCodec });

function settlementSender(options) {
  return new BatchedPortSender({
  port: self,
  wrap: (batch) => Tag("SettlementFrame", { batch }),
  maximumItems: MAXIMUM_WIRE_BATCH_ITEMS,
  maximumBytes: 1024 * 1024,
  scheduleTask: messageTaskScheduler(),
  failed: fail,
    ...options,
  });
}

const clonedCommandReceiver = commandReceiver(clonedSettlementSender);
const packedCommandReceiver = commandReceiver(packedSettlementSender);
const codecCommandReceiver = commandReceiver(codecSettlementSender, [workerInvocationCodec]);

function commandReceiver(sender, codecs = []) {
  return new BatchedPortReceiver((invocation) => {
    void settleCommand(invocation, sender);
  }, codecs);
}

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
    CloneCommand: async ({ invocation }) => {
      const outcome = await commandOutcome(invocation);
      self.postMessage(Tag("CloneSettlement", { id: invocation.id, outcome }));
    },
    TransferNumbers: ({ id, buffer }) => {
      self.postMessage(Tag("TransferredNumbers", { id, buffer }), [buffer]);
    },
    ClonedCommandFrame: ({ batch }) => clonedCommandReceiver.receive(batch),
    PackedCommandFrame: ({ batch }) => packedCommandReceiver.receive(batch),
    CodecCommandFrame: ({ batch }) => codecCommandReceiver.receive(batch),
  });
});

async function settleCommand(invocation, sender) {
  sender.send({
    id: invocation.id,
    call: invocation.call.tag,
    outcome: await commandOutcome(invocation),
  });
}

async function commandOutcome(invocation) {
  if (invocation.call.tag !== "Execute") {
    throw new Error("benchmark received a non-execute invocation");
  }
  return match(invocation.call.data, {
    Announce: () => Tag("Succeeded", Tag("Announced")),
    SendSinglePacket: ({ payload }) => Tag("Succeeded", Tag("PacketDelivered", {
      rttMillis: payload.byteLength / 10,
      evidence: "ExplicitProof",
    })),
    Request: ({ payload }) => Tag("Succeeded", Tag("ResponseReceived", {
      data: payload,
      rttMillis: payload.byteLength / 10,
    })),
    SendChannelMessage: ({ payload }) => Tag("Succeeded", Tag("PacketDelivered", {
      rttMillis: payload.byteLength / 10,
      evidence: "ImplicitProof",
    })),
    RequestPath: async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
      return Tag("Succeeded", Tag("PathDiscovered", { hops: 1 }));
    },
  });
}

function fail(error) {
  self.postMessage(Tag("Failed", { detail: String(error?.stack ?? error) }));
}
