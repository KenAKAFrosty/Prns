import {
  Prns,
  Tag,
  match_into,
} from "../src/native/index.js";
import type {
  ApplicationEvent,
  CommandOutcome,
  InterfaceId,
  StreamClaim,
} from "../src/native/index.js";

function claim<Value>(outcome: StreamClaim<Value>): AsyncIterableIterator<Value> {
  return match_into<AsyncIterableIterator<Value>>().from(outcome, {
    Claimed: (stream) => stream,
    AlreadyClaimed: ({ lane }) => {
      throw new Error(`${lane} already has an owner`);
    },
  });
}

function describe(event: ApplicationEvent): string {
  return match_into<string>().from(event, {
    SingleDelivery: ({ plaintext }) => `single packet: ${plaintext.length} bytes`,
    Request: ({ data }) => `request: ${data.length} bytes`,
    Response: ({ data }) => `response: ${data.length} bytes`,
    ResponseSegment: ({ segmentIndex, totalSegments }) =>
      `response segment ${segmentIndex + 1}/${totalSegments}`,
    ResourceAvailable: ({ resource }) => `resource: ${resource.totalBytes} bytes`,
    ResourceSegment: ({ segmentIndex, totalSegments }) =>
      `resource segment ${segmentIndex + 1}/${totalSegments}`,
    ResourceNeedsDecompression: ({ uncompressedDataBytes }) =>
      `compressed resource: ${uncompressedDataBytes} bytes`,
    ChannelMessage: ({ messageType }) => `channel message: ${messageType}`,
  });
}

function attachedInterface(outcome: CommandOutcome): InterfaceId | undefined {
  return match_into<InterfaceId | undefined>().from(outcome, {
    Announced: () => undefined,
    PacketDelivered: () => undefined,
    LinkCloseQueued: () => undefined,
    InterfaceAttached: ({ interface: interfaceId }) => interfaceId,
    InterfaceDetached: () => undefined,
    LinkEstablished: () => undefined,
    PathDiscovered: () => undefined,
    Identified: () => undefined,
    ResponseReceived: () => undefined,
    ResponseSent: () => undefined,
    ResourceSent: () => undefined,
    ResourceStrategySet: () => undefined,
    RequesterAllowed: () => undefined,
  });
}

const created = await Prns.create({
  identity: Tag("GenerateEphemeral"),
  role: "Endpoint",
});
if (created.tag !== "Ready") {
  throw new Error(`node creation failed: ${created.tag}`);
}
const node = created.data;
const events = claim(node.claimEvents());
const eventTask = (async () => {
  for await (const event of events) {
    console.log(describe(event));
  }
})();

const attached = await node.execute(
  Tag("AttachTcpClient", {
    target: "127.0.0.1:4242",
    bitrate: Tag("Auto"),
  }),
);
if (attached.tag !== "Succeeded") {
  throw new Error(`attach failed: ${attached.data.tag}`);
}
const interfaceOutcome = attachedInterface(attached.data);
if (interfaceOutcome === undefined) {
  throw new Error(`unexpected attach outcome: ${attached.data.tag}`);
}
const detached = await node.execute(
  Tag("DetachInterface", {
    interface: interfaceOutcome,
  }),
);
if (detached.tag !== "Succeeded") {
  throw new Error(`detach failed: ${detached.data.tag}`);
}
await node.stop();
await eventTask;
