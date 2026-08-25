import { Prns, Tag, match_into } from "../src/native/index.js";
import type { ApplicationEvent } from "../src/native/index.js";

async function runExample() {
  const creationOutcome = await Prns.create({
    identity: Tag("GenerateEphemeral"),
    role: "Endpoint",
  });

  if (creationOutcome.tag !== "Ready") {
    throw new Error(`node creation failed: ${creationOutcome.tag}`);
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

  try {
    const attached = await node.attachTcpServer({ bind: "127.0.0.1:0" });
    if (attached.tag !== "Succeeded") {
      throw new Error(`attach failed: ${attached.data.tag}`);
    }
    const interfaceId = attached.data.data.interface;

    console.log("attached loopback TCP server", interfaceId);

    const detached = await node.detachInterface(interfaceId);
    if (detached.tag !== "Succeeded") {
      throw new Error(`detach failed: ${detached.data.tag}`);
    }
  } finally {
    const stopped = await node.stop();
    if (stopped.tag === "OperationFailed") {
      throw new Error(`stop failed: ${stopped.data.detail}`);
    }
    await eventTask;
  }
}

function describe(event: ApplicationEvent): string {
  return match_into<string>().from(event, {
    SingleDelivery: ({ plaintext }) => `single packet: ${plaintext.length} bytes`,
    LinkDelivery: ({ plaintext }) => `Link packet: ${plaintext.length} bytes`,
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

await runExample();
