import {
  Prns,
  Tag,
  match_into,
} from "../src/native/index.js";
import type {
  ApplicationEvent,
} from "../src/native/index.js";

async function runExample() {
  const creationOutcome = await Prns.create({
    identity: Tag("GenerateEphemeral"),
    role: "Endpoint",
  });

  if (creationOutcome.tag !== "Ready") {
    throw new Error(`node creation failed: ${creationOutcome}`);
  }

  const node = creationOutcome.data;

  const claimOutcome = node.claimEvents();
  if (claimOutcome.tag === "AlreadyClaimed") {
    const lane = claimOutcome.data;
    throw new Error(`${lane} already has an owner`);
  }
  const events = claimOutcome.data;

  const eventTask = (async () => {
    for await (const event of events) {
      console.log(describe(event));
    }
  })();

  const attachedOutcome = await node.execute(
    Tag("AttachTcpClient", {
      target: "127.0.0.1:4242",
      bitrate: Tag("Auto"),
    }),
  );

  if (attachedOutcome.tag !== "Succeeded") {
    throw new Error(`attach failed: ${attachedOutcome}`);
  }

  const attachedTcpClient = attachedOutcome.data;

  const detached = await node.execute(
    Tag("DetachInterface", {
      interface: attachedTcpClient.data.interface,
    }),
  );
  if (detached.tag !== "Succeeded") {
    throw new Error(`detach failed: ${detached.data.tag}`);
  }

  await node.stop();
  await eventTask;
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


runExample()
