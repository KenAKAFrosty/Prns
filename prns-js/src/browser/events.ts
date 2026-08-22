import { Tag, match, match_into } from "../casework.js";
import {
  destinationHash,
  identityHash,
  interfaceId,
  linkId,
  requestId,
  requestPathHash,
  resourceHash,
} from "../contract.js";
import type {
  ApplicationEvent,
  CommandSettlement,
  DiagnosticEvent,
} from "../contract.js";
import { MemoryResourceStream } from "../memory_resource.js";
import { parseCommandSettlement } from "./command_settlement.js";
import {
  bigintField,
  bytesField,
  nonNegativeBigIntField,
  numberField,
  optionalBytesField,
  record,
  stringField,
} from "./decoding.js";
import {
  PrnsValidationError,
  commandId,
  copyBytes,
  hopCount,
  nonNegativeInteger,
  positiveInteger,
} from "./values.js";
import type { CommandId } from "./values.js";

export type AnnounceEvent = Extract<
  DiagnosticEvent,
  Tag<"AnnounceHeard", unknown>
>;
export type SingleDeliveryEvent = Extract<
  ApplicationEvent,
  Tag<"SingleDelivery", unknown>
>;
export type LinkDeliveryEvent = Extract<
  ApplicationEvent,
  Tag<"LinkDelivery", unknown>
>;
export type RequestEvent = Extract<
  ApplicationEvent,
  Tag<"Request", unknown>
>;
export type ResponseEvent = Extract<
  ApplicationEvent,
  Tag<"Response", unknown>
>;
export type ResponseSegmentEvent = Extract<
  ApplicationEvent,
  Tag<"ResponseSegment", unknown>
>;
export type ResourceAvailableEvent = Extract<
  ApplicationEvent,
  Tag<"ResourceAvailable", unknown>
>;
export type ResourceSegmentEvent = Extract<
  ApplicationEvent,
  Tag<"ResourceSegment", unknown>
>;
export type ChannelMessageEvent = Extract<
  ApplicationEvent,
  Tag<"ChannelMessage", unknown>
>;
export type RouteEvent = Extract<
  DiagnosticEvent,
  Tag<
    "RouteExpired" | "RouteEvicted" | "RouteInterfaceGone" | "RouteDropped",
    unknown
  >
>;
export type DiagnosticsDroppedEvent = Extract<
  DiagnosticEvent,
  Tag<"DiagnosticsDropped", unknown>
>;
export type LinkEvent = Extract<
  DiagnosticEvent,
  Tag<
    | "LinkEstablished"
    | "PeerIdentified"
    | "LinkClosed"
    | "LinkInterfaceMismatch",
    unknown
  >
>;
export type ResourceDiagnosticEvent = Extract<
  DiagnosticEvent,
  Tag<
    | "ResourceAssembled"
    | "ResourceFailed"
    | "ResourceSendProgress",
    unknown
  >
>;
export type RuntimeDiagnosticEvent = Extract<
  DiagnosticEvent,
  Tag<
    "SelfRatchetRotated" | "AnnounceHeldDropped" | "Delivered",
    unknown
  >
>;

export type PrnsApplicationEvent = ApplicationEvent;
export type PrnsDiagnosticEvent = DiagnosticEvent;
export type PrnsEvent = PrnsApplicationEvent | PrnsDiagnosticEvent;

type CommandSettledEvent = Tag<
  "CommandSettled",
  {
    readonly commandId: CommandId;
    readonly settlement?: CommandSettlement;
  }
>;

export type ParsedPrnsEvent =
  | Tag<"Application", PrnsApplicationEvent>
  | Tag<"Diagnostic", Exclude<PrnsDiagnosticEvent, DiagnosticsDroppedEvent>>
  | Tag<
      "CommandResponse",
      {
        readonly commandId: CommandId;
        readonly event: ResponseEvent;
      }
    >
  | Tag<
      "CommandResponseSegment",
      {
        readonly commandId: CommandId;
        readonly event: ResponseSegmentEvent;
      }
    >
  | CommandSettledEvent;

type RawEventType =
  | "announce"
  | "selfRatchetRotated"
  | "announceHeldDropped"
  | "commandSettled"
  | "linkEstablished"
  | "peerIdentified"
  | "request"
  | "response"
  | "responseSegment"
  | "channelMessage"
  | "singleDelivery"
  | "linkDelivery"
  | "delivered"
  | "linkClosed"
  | "linkInterfaceMismatch"
  | "resourceReceived"
  | "resourceFailed"
  | "resourceNeedsDecompression"
  | "resourceSegment"
  | "resourceAssembled"
  | "routeExpired"
  | "routeEvicted"
  | "routeInterfaceGone"
  | "routeDropped";

type RawEvent = {
  [Name in RawEventType]: Tag<Name, Record<string, unknown>>;
}[RawEventType];

type RawLinkClosedReason = "timeout" | "peerClosed" | "malformedRtt";

const RAW_EVENT_TYPES: ReadonlySet<string> = new Set<RawEventType>([
  "announce",
  "selfRatchetRotated",
  "announceHeldDropped",
  "commandSettled",
  "linkEstablished",
  "peerIdentified",
  "request",
  "response",
  "responseSegment",
  "channelMessage",
  "singleDelivery",
  "linkDelivery",
  "delivered",
  "linkClosed",
  "linkInterfaceMismatch",
  "resourceReceived",
  "resourceFailed",
  "resourceNeedsDecompression",
  "resourceSegment",
  "resourceAssembled",
  "routeExpired",
  "routeEvicted",
  "routeInterfaceGone",
  "routeDropped",
]);

const RAW_LINK_CLOSED_REASONS: ReadonlySet<string> =
  new Set<RawLinkClosedReason>([
    "timeout",
    "peerClosed",
    "malformedRtt",
  ]);

export function parseEvent(raw: unknown): ParsedPrnsEvent {
  const object = record(raw, "PrnsEvent");
  const event = Tag(
    rawEventType(stringField(object, "type")),
    object,
  ) as RawEvent;
  return match_into<ParsedPrnsEvent>().from(event, {
    announce: (data) =>
      Tag(
        "Diagnostic",
        Tag("AnnounceHeard", {
          appData: copyBytes(bytesField(data, "appData")),
          destination: destinationHash(bytesField(data, "destination")),
          hops: hopCount(numberField(data, "hops")),
          sourceInterface: interfaceId(bytesField(data, "sourceInterface")),
        }),
      ),
    selfRatchetRotated: (data) =>
      Tag(
        "Diagnostic",
        Tag("SelfRatchetRotated", {
          destination: destinationHash(bytesField(data, "destination")),
        }),
      ),
    announceHeldDropped: (data) =>
      Tag(
        "Diagnostic",
        Tag("AnnounceHeldDropped", {
          destination: destinationHash(bytesField(data, "destination")),
          sourceInterface: interfaceId(bytesField(data, "sourceInterface")),
          cause: stringField(data, "cause"),
        }),
      ),
    commandSettled: (data) => {
      const commandIdValue = commandId(bigintField(data, "id"));
      const settlement = parseCommandSettlement(data);
      return Tag(
        "CommandSettled",
        settlement === undefined
          ? { commandId: commandIdValue }
          : { commandId: commandIdValue, settlement },
      );
    },
    linkEstablished: (data) =>
      Tag(
        "Diagnostic",
        Tag("LinkEstablished", {
          linkId: linkId(bytesField(data, "linkId")),
          rttMillis: nonNegativeInteger(
            numberField(data, "rttMillis"),
            "rttMillis",
          ),
        }),
      ),
    peerIdentified: (data) =>
      Tag(
        "Diagnostic",
        Tag("PeerIdentified", {
          linkId: linkId(bytesField(data, "linkId")),
          identity: identityHash(bytesField(data, "identity")),
        }),
      ),
    request: (data) => {
      const request = {
        destination: destinationHash(bytesField(data, "destination")),
        linkId: linkId(bytesField(data, "linkId")),
        requestId: requestId(bytesField(data, "requestId")),
        pathHash: requestPathHash(bytesField(data, "pathHash")),
        rttMillis: nonNegativeInteger(
          numberField(data, "rttMillis"),
          "rttMillis",
        ),
        data: copyBytes(bytesField(data, "data")),
      };
      const requester = optionalBytesField(data, "requester");
      return Tag(
        "Application",
        Tag(
          "Request",
          requester
            ? { ...request, requester: identityHash(requester) }
            : request,
        ),
      );
    },
    response: (data) => {
      const responseCommandId = commandId(bigintField(data, "commandId"));
      return Tag("CommandResponse", {
        commandId: responseCommandId,
        event: Tag("Response", {
          linkId: linkId(bytesField(data, "linkId")),
          requestId: requestId(bytesField(data, "requestId")),
          data: copyBytes(bytesField(data, "data")),
        }),
      });
    },
    responseSegment: (data) => {
      const responseCommandId = commandId(bigintField(data, "commandId"));
      return Tag("CommandResponseSegment", {
        commandId: responseCommandId,
        event: Tag("ResponseSegment", {
          linkId: linkId(bytesField(data, "linkId")),
          requestId: requestId(bytesField(data, "requestId")),
          segmentIndex: nonNegativeInteger(
            numberField(data, "segmentIndex"),
            "segmentIndex",
          ),
          totalSegments: positiveInteger(
            numberField(data, "totalSegments"),
            "totalSegments",
          ),
          data: copyBytes(bytesField(data, "data")),
        }),
      });
    },
    channelMessage: (data) =>
      Tag(
        "Application",
        Tag("ChannelMessage", {
          linkId: linkId(bytesField(data, "linkId")),
          messageType: nonNegativeInteger(
            numberField(data, "messageType"),
            "messageType",
          ),
          data: copyBytes(bytesField(data, "data")),
        }),
      ),
    singleDelivery: (data) =>
      Tag(
        "Application",
        Tag("SingleDelivery", {
          destination: destinationHash(bytesField(data, "destination")),
          plaintext: copyBytes(bytesField(data, "plaintext")),
          sourceInterface: interfaceId(bytesField(data, "sourceInterface")),
        }),
      ),
    linkDelivery: (data) =>
      Tag(
        "Application",
        Tag("LinkDelivery", {
          linkId: linkId(bytesField(data, "linkId")),
          plaintext: copyBytes(bytesField(data, "plaintext")),
          sourceInterface: interfaceId(bytesField(data, "sourceInterface")),
        }),
      ),
    delivered: (data) =>
      Tag(
        "Diagnostic",
        Tag("Delivered", { detail: stringField(data, "detail") }),
      ),
    linkClosed: (data) =>
      Tag(
        "Diagnostic",
        Tag("LinkClosed", {
          linkId: linkId(bytesField(data, "linkId")),
          reason: linkClosedReason(stringField(data, "reason")),
        }),
      ),
    linkInterfaceMismatch: (data) =>
      Tag(
        "Diagnostic",
        Tag("LinkInterfaceMismatch", {
          linkId: linkId(bytesField(data, "linkId")),
          attachedInterface: interfaceId(
            bytesField(data, "attachedInterface"),
          ),
          arrivedOn: interfaceId(bytesField(data, "arrivedOn")),
        }),
      ),
    resourceReceived: (data) => {
      const details = {
        linkId: linkId(bytesField(data, "linkId")),
        hash: resourceHash(bytesField(data, "hash")),
        resource: new MemoryResourceStream(bytesField(data, "data")),
      };
      const metadata = optionalBytesField(data, "metadata");
      return Tag(
        "Application",
        Tag(
          "ResourceAvailable",
          metadata
            ? { ...details, metadata: copyBytes(metadata) }
            : details,
        ),
      );
    },
    resourceFailed: (data) =>
      Tag(
        "Diagnostic",
        Tag("ResourceFailed", {
          linkId: linkId(bytesField(data, "linkId")),
          hash: resourceHash(bytesField(data, "hash")),
          cause: stringField(data, "cause"),
        }),
      ),
    resourceNeedsDecompression: (data) =>
      Tag(
        "Application",
        Tag("ResourceNeedsDecompression", {
          linkId: linkId(bytesField(data, "linkId")),
          hash: resourceHash(bytesField(data, "hash")),
          stream: copyBytes(bytesField(data, "stream")),
          uncompressedDataBytes: nonNegativeBigIntField(
            data,
            "uncompressedDataBytes",
          ),
        }),
      ),
    resourceSegment: (data) => {
      const details = {
        linkId: linkId(bytesField(data, "linkId")),
        originalHash: resourceHash(bytesField(data, "originalHash")),
        segmentIndex: nonNegativeInteger(
          numberField(data, "segmentIndex"),
          "segmentIndex",
        ),
        totalSegments: positiveInteger(
          numberField(data, "totalSegments"),
          "totalSegments",
        ),
        data: copyBytes(bytesField(data, "data")),
      };
      const metadata = optionalBytesField(data, "metadata");
      return Tag(
        "Application",
        Tag(
          "ResourceSegment",
          metadata
            ? { ...details, metadata: copyBytes(metadata) }
            : details,
        ),
      );
    },
    resourceAssembled: (data) =>
      Tag(
        "Diagnostic",
        Tag("ResourceAssembled", {
          linkId: linkId(bytesField(data, "linkId")),
          originalHash: resourceHash(bytesField(data, "originalHash")),
          totalSizeBytes: nonNegativeBigIntField(data, "totalSizeBytes"),
        }),
      ),
    routeExpired: (data) =>
      Tag(
        "Diagnostic",
        Tag("RouteExpired", {
          destination: destinationHash(bytesField(data, "destination")),
        }),
      ),
    routeEvicted: (data) =>
      Tag(
        "Diagnostic",
        Tag("RouteEvicted", {
          destination: destinationHash(bytesField(data, "destination")),
        }),
      ),
    routeInterfaceGone: (data) =>
      Tag(
        "Diagnostic",
        Tag("RouteInterfaceGone", {
          destination: destinationHash(bytesField(data, "destination")),
        }),
      ),
    routeDropped: (data) =>
      Tag(
        "Diagnostic",
        Tag("RouteDropped", {
          destination: destinationHash(bytesField(data, "destination")),
        }),
      ),
  });
}

function rawEventType(value: string): RawEventType {
  if (!RAW_EVENT_TYPES.has(value)) {
    throw new PrnsValidationError(
      "invalid-component",
      `runtime emitted event outside host contract: ${value}`,
    );
  }
  return value as RawEventType;
}

function linkClosedReason(
  value: string,
): "Timeout" | "PeerClosed" | "MalformedRtt" {
  if (!RAW_LINK_CLOSED_REASONS.has(value)) {
    throw new PrnsValidationError(
      "invalid-component",
      `unknown link close reason ${value}`,
    );
  }
  return match(value as RawLinkClosedReason, {
    timeout: () => "Timeout" as const,
    peerClosed: () => "PeerClosed" as const,
    malformedRtt: () => "MalformedRtt" as const,
  });
}
