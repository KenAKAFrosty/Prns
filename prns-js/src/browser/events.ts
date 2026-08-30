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
import {
  APPLICATION_EVENT_KIND_CODES,
  DIAGNOSTIC_EVENT_KIND_CODES,
  EVENT_FIELD_CODES,
} from "../contract.generated.js";
import {
  decodeEventBatchProjection,
} from "../event_projection.js";
import type {
  EventProjection,
  EventProjectionValue,
} from "../event_projection.js";
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
    | "SelfRatchetRotated"
    | "AnnounceHeldDropped"
    | "Delivered"
    | "BackendDiagnostic",
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
  | "backendDiagnostic"
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
  | "routeDropped"
  | "persistenceFlushed"
  | "persistenceFlushFailed";

type RawEvent = {
  [Name in RawEventType]: Tag<Name, Record<string, unknown>>;
}[RawEventType];

type RawLinkClosedReason =
  | "timeout"
  | "peerClosed"
  | "malformedRtt"
  | "locallyClosed";

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
  "backendDiagnostic",
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
  "persistenceFlushed",
  "persistenceFlushFailed",
]);

const RAW_LINK_CLOSED_REASONS: ReadonlySet<string> =
  new Set<RawLinkClosedReason>([
    "timeout",
    "peerClosed",
    "malformedRtt",
    "locallyClosed",
  ]);

const COMMAND_ID_PROJECTION_FIELD = 32_768;
const CORRELATED_EVENT_KINDS: ReadonlySet<number> = new Set([
  APPLICATION_EVENT_KIND_CODES.Response,
  APPLICATION_EVENT_KIND_CODES.ResponseSegment,
]);
const DIAGNOSTIC_EVENT_KINDS_SET: ReadonlySet<number> = new Set(
  Object.values(DIAGNOSTIC_EVENT_KIND_CODES),
);

export function parseEventBatch(bytes: Uint8Array): ParsedPrnsEvent[] {
  return decodeEventBatchProjection(bytes).map((event) =>
    parseEvent(rawProjectedEvent(event))
  );
}

export function parseDiagnosticEventBatch(
  bytes: Uint8Array,
): PrnsDiagnosticEvent[] {
  const diagnostics: PrnsDiagnosticEvent[] = [];
  for (const projected of decodeEventBatchProjection(
    bytes,
    DIAGNOSTIC_EVENT_KINDS_SET,
  )) {
    match(parseEvent(rawProjectedEvent(projected)), {
      Diagnostic: (diagnostic) => diagnostics.push(diagnostic),
      Application: () => undefined,
      CommandResponse: () => undefined,
      CommandResponseSegment: () => undefined,
      CommandSettled: () => undefined,
    });
  }
  return diagnostics;
}

export function parseCorrelatedEventBatch(bytes: Uint8Array): ParsedPrnsEvent[] {
  return decodeEventBatchProjection(bytes, CORRELATED_EVENT_KINDS)
    .map((event) => parseEvent(rawProjectedEvent(event)));
}

export function retainedApplicationEventBytes(
  event: PrnsApplicationEvent,
): number {
  return match_into<number>().from(event, {
    SingleDelivery: ({ plaintext }) => plaintext.length,
    LinkDelivery: ({ plaintext }) => plaintext.length,
    Request: ({ data }) => data.length,
    Response: ({ data }) => data.length,
    ResponseSegment: ({ data }) => data.length,
    ResourceAvailable: ({ resource, metadata }) =>
      exactBytesAsSafeNumber(resource.totalBytes, "resource.totalBytes") +
      (metadata?.length ?? 0),
    ResourceSegment: ({ data, metadata }) => data.length + (metadata?.length ?? 0),
    ResourceNeedsDecompression: ({ stream }) => stream.length,
    ChannelMessage: ({ data }) => data.length,
  });
}

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
    backendDiagnostic: (data) =>
      Tag(
        "Diagnostic",
        Tag("BackendDiagnostic", {
          kind: stringField(data, "kind"),
          detail: stringField(data, "detail"),
        }),
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
    persistenceFlushed: (data) =>
      Tag(
        "Diagnostic",
        Tag("PersistenceFlushed", {
          cause: persistenceCause(stringField(data, "cause")),
          target: persistenceTarget(stringField(data, "target")),
        }),
      ),
    persistenceFlushFailed: (data) =>
      Tag(
        "Diagnostic",
        Tag("PersistenceFlushFailed", {
          cause: persistenceCause(stringField(data, "cause")),
          target: persistenceTarget(stringField(data, "target")),
        }),
      ),
  });
}

function rawProjectedEvent(event: EventProjection): Record<string, unknown> {
  const raw = projectedEventFields(event);
  switch (event.kind) {
    case APPLICATION_EVENT_KIND_CODES.SingleDelivery:
      return { type: "singleDelivery", ...raw };
    case APPLICATION_EVENT_KIND_CODES.Request:
      return { type: "request", ...raw };
    case APPLICATION_EVENT_KIND_CODES.Response:
      return { type: "response", ...raw };
    case APPLICATION_EVENT_KIND_CODES.ResponseSegment:
      return { type: "responseSegment", ...raw };
    case APPLICATION_EVENT_KIND_CODES.ResourceAvailable:
      return { type: "resourceReceived", ...raw };
    case APPLICATION_EVENT_KIND_CODES.ResourceSegment:
      return { type: "resourceSegment", ...raw };
    case APPLICATION_EVENT_KIND_CODES.ResourceNeedsDecompression:
      return { type: "resourceNeedsDecompression", ...raw };
    case APPLICATION_EVENT_KIND_CODES.ChannelMessage:
      return { type: "channelMessage", ...raw };
    case APPLICATION_EVENT_KIND_CODES.LinkDelivery:
      return { type: "linkDelivery", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.AnnounceHeard:
      return { type: "announce", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.LinkEstablished:
      return { type: "linkEstablished", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.PeerIdentified:
      return { type: "peerIdentified", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.LinkClosed:
      return { type: "linkClosed", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.LinkInterfaceMismatch:
      return { type: "linkInterfaceMismatch", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.ResourceAssembled:
      return { type: "resourceAssembled", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.ResourceFailed:
      return { type: "resourceFailed", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.SelfRatchetRotated:
      return { type: "selfRatchetRotated", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.AnnounceHeldDropped:
      return { type: "announceHeldDropped", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.Delivered:
      return { type: "delivered", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.BackendDiagnostic:
      return { type: "backendDiagnostic", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.RouteExpired:
      return { type: "routeExpired", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.RouteEvicted:
      return { type: "routeEvicted", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.RouteInterfaceGone:
      return { type: "routeInterfaceGone", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.RouteDropped:
      return { type: "routeDropped", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.PersistenceFlushed:
      return { type: "persistenceFlushed", ...raw };
    case DIAGNOSTIC_EVENT_KIND_CODES.PersistenceFlushFailed:
      return { type: "persistenceFlushFailed", ...raw };
    default:
      throw new PrnsValidationError(
        "invalid-component",
        `runtime emitted projected event kind outside host contract: ${event.kind}`,
      );
  }
}

function projectedEventFields(
  event: EventProjection,
): Record<string, unknown> {
  const fields: Record<string, unknown> = {};
  for (const [id, value] of event.fields) {
    const name = projectedFieldName(id);
    fields[name] = projectedFieldValue(name, value);
  }
  return fields;
}

function projectedFieldName(id: number): string {
  switch (id) {
    case EVENT_FIELD_CODES.Destination:
      return "destination";
    case EVENT_FIELD_CODES.SourceInterface:
      return "sourceInterface";
    case EVENT_FIELD_CODES.Plaintext:
      return "plaintext";
    case EVENT_FIELD_CODES.LinkId:
      return "linkId";
    case EVENT_FIELD_CODES.RequestId:
      return "requestId";
    case EVENT_FIELD_CODES.Requester:
      return "requester";
    case EVENT_FIELD_CODES.PathHash:
      return "pathHash";
    case EVENT_FIELD_CODES.RttMillis:
      return "rttMillis";
    case EVENT_FIELD_CODES.Data:
      return "data";
    case EVENT_FIELD_CODES.SegmentIndex:
      return "segmentIndex";
    case EVENT_FIELD_CODES.TotalSegments:
      return "totalSegments";
    case EVENT_FIELD_CODES.Hash:
      return "hash";
    case EVENT_FIELD_CODES.OriginalHash:
      return "originalHash";
    case EVENT_FIELD_CODES.Metadata:
      return "metadata";
    case EVENT_FIELD_CODES.UncompressedDataBytes:
      return "uncompressedDataBytes";
    case EVENT_FIELD_CODES.MessageType:
      return "messageType";
    case EVENT_FIELD_CODES.Identity:
      return "identity";
    case EVENT_FIELD_CODES.Reason:
      return "reason";
    case EVENT_FIELD_CODES.AttachedInterface:
      return "attachedInterface";
    case EVENT_FIELD_CODES.ArrivedOn:
      return "arrivedOn";
    case EVENT_FIELD_CODES.TotalSizeBytes:
      return "totalSizeBytes";
    case EVENT_FIELD_CODES.Cause:
      return "cause";
    case EVENT_FIELD_CODES.Detail:
      return "detail";
    case EVENT_FIELD_CODES.Kind:
      return "kind";
    case EVENT_FIELD_CODES.Hops:
      return "hops";
    case EVENT_FIELD_CODES.Stream:
      return "stream";
    case EVENT_FIELD_CODES.PersistenceCause:
      return "cause";
    case EVENT_FIELD_CODES.PersistenceTarget:
      return "target";
    case EVENT_FIELD_CODES.AppData:
      return "appData";
    case COMMAND_ID_PROJECTION_FIELD:
      return "commandId";
    default:
      throw new PrnsValidationError(
        "invalid-component",
        `runtime emitted projected field outside host contract: ${id}`,
      );
  }
}

function projectedFieldValue(
  name: string,
  value: EventProjectionValue,
): EventProjectionValue | number {
  if (
    typeof value !== "bigint" ||
    name === "commandId" ||
    name === "uncompressedDataBytes" ||
    name === "totalSizeBytes"
  ) {
    return value;
  }
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new PrnsValidationError(
      "invalid-number",
      `runtime emitted ${name} outside JavaScript's safe integer range`,
    );
  }
  return Number(value);
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
): "Timeout" | "PeerClosed" | "MalformedRtt" | "LocallyClosed" {
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
    locallyClosed: () => "LocallyClosed" as const,
  });
}

function exactBytesAsSafeNumber(value: bigint, name: string): number {
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new PrnsValidationError(
      "invalid-number",
      `${name} exceeds the JavaScript safe-integer limit`,
    );
  }
  return Number(value);
}

function persistenceCause(
  value: string,
): "Startup" | "Interval" | "RouteChange" | "RatchetRotation" | "Shutdown" {
  switch (value) {
    case "startup":
      return "Startup";
    case "interval":
      return "Interval";
    case "route_change":
      return "RouteChange";
    case "ratchet_rotation":
      return "RatchetRotation";
    case "shutdown":
      return "Shutdown";
    default:
      throw new PrnsValidationError(
        "invalid-component",
        `unknown persistence flush cause ${value}`,
      );
  }
}

function persistenceTarget(value: string): "RoutingState" | "Ratchets" {
  switch (value) {
    case "routing_state":
      return "RoutingState";
    case "ratchets":
      return "Ratchets";
    default:
      throw new PrnsValidationError(
        "invalid-component",
        `unknown persistence flush target ${value}`,
      );
  }
}
