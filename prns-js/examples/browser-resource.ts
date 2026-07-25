import {
  Prns,
  Tag,
  match_into,
} from "../src/browser/index.js";
import type {
  CommandFailure,
  LinkId,
  PrnsWasmModule,
} from "../src/browser/index.js";

function describeFailure(failure: CommandFailure): string {
  return match_into<string>().from(failure, {
    NodeStopped: () => "node stopped",
    Busy: () => "node is busy",
    PayloadTooLarge: () => "payload is too large",
    UnknownDestination: () => "unknown destination",
    NotSingleDestination: () => "destination is not single",
    AnnounceAppDataTooLong: () => "announce app data is too long",
    UnknownInterface: () => "unknown interface",
    NoRouteToDestination: () => "no route to destination",
    NotDirectlyReachable: () => "destination is not directly reachable",
    PacketCulled: () => "packet was culled",
    DeliveryTimedOut: () => "delivery timed out",
    InvalidBitrate: () => "invalid bitrate",
    BindFailed: ({ detail }) => `bind failed: ${detail}`,
    WriteFailed: ({ detail }) => `write failed: ${detail}`,
    UnsupportedByBackend: () => "operation is unavailable in this browser backend",
    UnknownLink: () => "unknown link",
    LinkNotActive: () => "link is not active",
    EntropyUnavailable: () => "entropy is unavailable",
    NotLinkInitiator: () => "node did not initiate the link",
    IdentityNotHeld: () => "identity is not held",
    UnknownRequestHandler: () => "unknown request handler",
    RequestPolicyNotAllowList: () => "request policy is not an allow list",
    RequestAllowListFull: () => "request allow list is full",
    LinkBusy: () => "link is busy",
    ResourceTableFull: () => "resource table full",
    ResourceMetadataTooLarge: () => "resource metadata too large",
    ResourceRejectedByPeer: () => "resource rejected",
    ResourceSequencingFailed: () => "resource sequencing failed",
    ResourcePredecessorFailed: () => "resource predecessor failed",
    ChannelWindowFull: () => "channel window is full",
    ChannelUntrackable: () => "channel cannot track the message",
    InvalidChannelMessageType: () => "invalid channel message type",
  });
}

export async function sendFile(
  wasm: PrnsWasmModule,
  linkId: LinkId,
  file: Blob,
): Promise<void> {
  const created = await Prns.create({ wasm });
  if (created.tag !== "Ready") {
    throw new Error(`browser node creation failed: ${created.tag}`);
  }
  const node = created.data;
  const sent = await node.sendResourceBlob(linkId, file, {
    compression: Tag("Auto"),
  });
  if (sent.tag === "Failed") {
    throw new Error(describeFailure(sent.data));
  }
  match_into<void>().from(sent.data, {
    ResourceSent: () => {},
  });
}
