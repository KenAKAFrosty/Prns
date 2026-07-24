import type {
  ApplicationEvent,
  BackendCapabilities,
  BackendStartFailed,
  CapabilityName,
  ContractMismatch,
  DestinationConfig,
  DestinationHash,
  DiagnosticEvent,
  IdentityConfig,
  InterfaceId,
  LifecycleState,
  LinkId,
  PrnsCreateOptions,
  PrnsLimits,
} from "../contract.js";
import type { StreamClaim } from "../async_lanes.js";
import type { Tag as Tagged } from "../casework.js";

type Buffer = Uint8Array;

declare const Buffer: {
  from(bytes: Uint8Array): Buffer;
};

declare function require(path: string): unknown;

const casework = require("../../dist-cjs/casework.js") as typeof import("../casework.js");
const contract = require("../../dist-cjs/contract.js") as typeof import("../contract.js");
const lanes = require("../../dist-cjs/async_lanes.js") as typeof import("../async_lanes.js");
const resources = require("../../dist-cjs/memory_resource.js") as typeof import("../memory_resource.js");
const addon = require("../../native/addon.cjs") as NativeBinding;

export const {
  Tag,
  from,
  match,
  match_into,
} = casework;
export type Tag<Name extends string, Data = undefined> = import("../casework.js").Tag<
  Name,
  Data
>;
export const {
  HOST_CONTRACT_ABI,
  PRODUCT_VERSION,
  DESTINATION_HASH_LENGTH,
  IDENTITY_HASH_LENGTH,
  INTERFACE_ID_LENGTH,
  LINK_ID_LENGTH,
  REQUEST_ID_LENGTH,
  REQUEST_PATH_HASH_LENGTH,
  RESOURCE_HASH_LENGTH,
  IDENTITY_SECRET_LENGTH,
  PrnsValidationError,
  balancedLimits,
  destinationHash,
  identityHash,
  interfaceId,
  linkId,
  requestId,
  requestPathHash,
  resourceHash,
  identitySecret,
} = contract;
export type {
  ApplicationEvent,
  BackendCapabilities,
  BackendStartFailed,
  CapabilityName,
  ContractMismatch,
  DestinationConfig,
  DestinationHash,
  DiagnosticEvent,
  IdentityConfig,
  IdentityHash,
  IdentitySecret,
  InterfaceId,
  LifecycleState,
  LinkId,
  PrnsCreateOptions,
  PrnsLimits,
  PrnsValidationCode,
  RequestId,
  RequestPathHash,
  ResourceHash,
  ResourceStream,
} from "../contract.js";
export type {
  DataFrom,
  Tag as Tagged,
  TagFrom,
} from "../casework.js";
export type { StreamClaim } from "../async_lanes.js";

type RawIdentity = {
  secret?: Buffer;
  path?: string;
};

type RawDestination = {
  appName: string;
  aspects: string[];
  kind: "single" | "plain";
  identity?: RawIdentity;
  announceAppData?: Buffer;
};

type RawNodeOptions = {
  identity?: RawIdentity;
  transport?: boolean;
  destinations?: RawDestination[];
  eventQueueLimit?: number;
  applicationEventQueueLimit?: number;
  retainedEventBytesLimit?: number;
  diagnosticEventQueueLimit?: number;
};

type RawNode = {
  readonly destinationHashes: Buffer[];
  ready(): Promise<void>;
  stop(): Promise<void>;
  announce(destination: Buffer): Promise<void>;
  sendSinglePacket(
    destination: Buffer,
    data: Buffer,
  ): Promise<{ rttMillis: number; evidence: string; packetHash?: Buffer }>;
  closeLink(linkId: Buffer): boolean;
  attachTcpServer(options: {
    bind: string;
    bitrateBps?: number;
  }): Promise<RawInterface>;
  attachTcpClient(options: {
    target: string;
    bitrateBps?: number;
  }): Promise<RawInterface>;
  attachUdp(options: {
    local: string;
    peer: string;
    bitrateBps?: number;
  }): Promise<RawInterface>;
};

type RawInterface = {
  readonly id: Buffer;
  readonly kind: string | null;
  teardown(): boolean;
};

type NativeBinding = {
  version(): string;
  hostContractAbi(): number;
  startNode(options: RawNodeOptions, onEvent: (event: unknown) => void): RawNode;
};

export type Busy = Tagged<"Busy">;
export type NodeStopped = Tagged<"NodeStopped">;
export type OperationFailed = Tagged<
  "OperationFailed",
  { readonly operation: string; readonly detail: string; readonly code?: string }
>;
export type CommandFailure = Busy | NodeStopped | OperationFailed;
export type StopOutcome =
  | Tagged<"Stopped">
  | Tagged<"AlreadyStopped">
  | OperationFailed;
export type AnnounceOutcome = Tagged<"Announced"> | CommandFailure;
export type SendSinglePacketOutcome =
  | Tagged<
      "Sent",
      {
        readonly rttMillis: number;
        readonly evidence: string;
        readonly packetHash?: Uint8Array;
      }
    >
  | CommandFailure;
export type AttachOutcome =
  | Tagged<"Attached", NativeInterface>
  | CommandFailure;
export type PrnsCreateOutcome =
  | Tagged<"Ready", Prns>
  | ContractMismatch
  | BackendStartFailed;

const NATIVE_CAPABILITIES: ReadonlySet<CapabilityName> = new Set([
  "Loopback",
  "TcpClient",
  "TcpServer",
  "Udp",
  "Serial",
  "Usb",
  "Bluetooth",
  "Wifi",
  "WebSocket",
  "BrowserRendezvous",
  "I2p",
  "Weave",
]);

export class NativeInterface {
  readonly id: InterfaceId;
  readonly kind: string | undefined;
  readonly #raw: RawInterface;

  constructor(raw: RawInterface) {
    this.#raw = raw;
    this.id = contract.interfaceId(raw.id);
    this.kind = raw.kind ?? undefined;
  }

  close(): Tagged<"Closed"> | Tagged<"AlreadyClosed"> {
    return this.#raw.teardown()
      ? casework.Tag("Closed")
      : casework.Tag("AlreadyClosed");
  }
}

export class Prns {
  readonly capabilities: BackendCapabilities = casework.Tag("Native", {
    available: NATIVE_CAPABILITIES,
  });
  readonly #limits: PrnsLimits;
  readonly #events: import("../async_lanes.js").BoundedAsyncLane<ApplicationEvent>;
  readonly #diagnostics: import("../async_lanes.js").BoundedAsyncLane<DiagnosticEvent>;
  readonly #raw: RawNode;
  #lifecycle: LifecycleState = casework.Tag("Starting");
  #pendingCommands = 0;

  private constructor(raw: RawNode, limits: PrnsLimits) {
    this.#raw = raw;
    this.#limits = limits;
    this.#events = new lanes.BoundedAsyncLane<ApplicationEvent>({
      name: "ApplicationEvents",
      maximumValues: limits.applicationEvents,
      maximumBytes: limits.retainedEventBytes,
      measure: retainedEventBytes,
      onRejected: (rejectedEventBytes) =>
        this.#failBackpressure(rejectedEventBytes),
    });
    this.#diagnostics = new lanes.BoundedAsyncLane<DiagnosticEvent>({
      name: "Diagnostics",
      maximumValues: limits.diagnostics,
      maximumBytes: Number.MAX_SAFE_INTEGER,
      measure: () => 0,
      gap: (count) => casework.Tag("DiagnosticsDropped", { count }),
    });
  }

  static create(options: PrnsCreateOptions): Promise<PrnsCreateOutcome> {
    const validated = validateCreateOptions(options);
    const actualAbi =
      typeof addon.hostContractAbi === "function" ? addon.hostContractAbi() : 0;
    const actualProductVersion = addon.version();
    if (
      actualAbi !== contract.HOST_CONTRACT_ABI ||
      actualProductVersion !== contract.PRODUCT_VERSION
    ) {
      return Promise.resolve(
        casework.Tag("ContractMismatch", {
          requiredAbi: contract.HOST_CONTRACT_ABI,
          actualAbi,
          requiredProductVersion: contract.PRODUCT_VERSION,
          actualProductVersion,
        }),
      );
    }
    let instance: Prns | undefined;
    try {
      const raw = addon.startNode(validated.raw, (event) => {
        instance?.handleRawEvent(event);
      });
      instance = new Prns(raw, validated.limits);
    } catch (error) {
      return Promise.resolve(backendStartFailed(error));
    }
    return instance.finishStarting();
  }

  get destinationHashes(): readonly DestinationHash[] {
    return this.#raw.destinationHashes.map((hash) =>
      contract.destinationHash(hash),
    );
  }

  get lifecycle(): LifecycleState {
    return this.#lifecycle;
  }

  claimEvents(): StreamClaim<ApplicationEvent> {
    return this.#events.claim();
  }

  claimDiagnostics(): StreamClaim<DiagnosticEvent> {
    return this.#diagnostics.claim();
  }

  async stop(): Promise<StopOutcome> {
    if (this.#lifecycle.tag === "Stopped") {
      return casework.Tag("AlreadyStopped");
    }
    if (this.#lifecycle.tag !== "Failed") {
      this.#lifecycle = casework.Tag("Stopping");
    }
    try {
      await this.#raw.stop();
      if (this.#lifecycle.tag !== "Failed") {
        this.#lifecycle = casework.Tag("Stopped", { reason: "Requested" });
      }
      this.#events.finish();
      this.#diagnostics.finish();
      return casework.Tag("Stopped");
    } catch (error) {
      const failure = operationFailed("stop", error);
      this.#failBackend(failure.data.detail);
      return failure;
    }
  }

  announce(destination: DestinationHash): Promise<AnnounceOutcome> {
    return this.runCommand("announce", async () => {
      await this.#raw.announce(Buffer.from(destination));
      return casework.Tag("Announced");
    });
  }

  sendSinglePacket(
    destination: DestinationHash,
    data: Uint8Array,
  ): Promise<SendSinglePacketOutcome> {
    const payload = bytes("data", data);
    return this.runCommand("sendSinglePacket", async () => {
      const receipt = await this.#raw.sendSinglePacket(
        Buffer.from(destination),
        Buffer.from(payload),
      );
      const result: {
        rttMillis: number;
        evidence: string;
        packetHash?: Uint8Array;
      } = {
        rttMillis: finiteNonNegative("rttMillis", receipt.rttMillis),
        evidence: receipt.evidence,
      };
      if (receipt.packetHash) {
        result.packetHash = Uint8Array.from(receipt.packetHash);
      }
      return casework.Tag("Sent", result);
    });
  }

  closeLink(link: LinkId): Tagged<"Closed"> | NodeStopped {
    if (isStopped(this.#lifecycle)) {
      return casework.Tag("NodeStopped");
    }
    return this.#raw.closeLink(Buffer.from(link))
      ? casework.Tag("Closed")
      : casework.Tag("NodeStopped");
  }

  attachTcpServer(options: {
    readonly bind: string;
    readonly bitrateBps?: number;
  }): Promise<AttachOutcome> {
    const raw = optionalBitrate({ bind: nonEmpty("bind", options.bind) }, options.bitrateBps);
    return this.runCommand("attachTcpServer", async () =>
      casework.Tag("Attached", new NativeInterface(await this.#raw.attachTcpServer(raw))),
    );
  }

  attachTcpClient(options: {
    readonly target: string;
    readonly bitrateBps?: number;
  }): Promise<AttachOutcome> {
    const raw = optionalBitrate(
      { target: nonEmpty("target", options.target) },
      options.bitrateBps,
    );
    return this.runCommand("attachTcpClient", async () =>
      casework.Tag("Attached", new NativeInterface(await this.#raw.attachTcpClient(raw))),
    );
  }

  attachUdp(options: {
    readonly local: string;
    readonly peer: string;
    readonly bitrateBps?: number;
  }): Promise<AttachOutcome> {
    const raw = optionalBitrate(
      {
        local: nonEmpty("local", options.local),
        peer: nonEmpty("peer", options.peer),
      },
      options.bitrateBps,
    );
    return this.runCommand("attachUdp", async () =>
      casework.Tag("Attached", new NativeInterface(await this.#raw.attachUdp(raw))),
    );
  }

  handleRawEvent(raw: unknown): void {
    const parsed = parseRawEvent(raw);
    casework.match(parsed, {
      Application: (event) => {
        this.#events.push(event);
      },
      Diagnostic: (event) => {
        this.#diagnostics.push(event);
      },
      BackpressureExceeded: ({ rejectedEventBytes }) => {
        this.#failBackpressure(rejectedEventBytes);
      },
      Stopped: ({ cause }) => {
        if (cause !== "stopped") {
          this.#failBackend(cause);
          return;
        }
        if (!isStopped(this.#lifecycle)) {
          this.#lifecycle = casework.Tag("Stopped", {
            reason: "BackendExited",
          });
        }
        this.#events.finish();
        this.#diagnostics.finish();
      },
      CommandSettled: () => undefined,
      ContractViolation: ({ detail }) => {
        this.#failBackend(detail);
      },
    });
  }

  async finishStarting(): Promise<PrnsCreateOutcome> {
    try {
      await this.#raw.ready();
      this.#lifecycle = casework.Tag("Running");
      return casework.Tag("Ready", this);
    } catch (error) {
      const failed = backendStartFailed(error);
      this.#failBackend(failed.data.detail);
      await this.#raw.stop().catch(() => undefined);
      return failed;
    }
  }

  async runCommand<Success>(
    operation: string,
    run: () => Promise<Success>,
  ): Promise<Success | CommandFailure> {
    if (isStopped(this.#lifecycle)) {
      return casework.Tag("NodeStopped");
    }
    if (this.#pendingCommands >= this.#limits.pendingCommands) {
      return casework.Tag("Busy");
    }
    this.#pendingCommands += 1;
    try {
      return await run();
    } catch (error) {
      return operationFailed(operation, error);
    } finally {
      this.#pendingCommands -= 1;
    }
  }

  #failBackpressure(rejectedEventBytes: number): void {
    if (isStopped(this.#lifecycle)) {
      return;
    }
    this.#lifecycle = casework.Tag("Failed", {
      cause: "EventBackpressureExceeded",
      limits: this.#limits,
      rejectedEventBytes,
    });
    this.#events.finish();
    this.#diagnostics.finish();
    queueMicrotask(() => {
      void this.#raw.stop().catch(() => undefined);
    });
  }

  #failBackend(detail: string): void {
    if (isStopped(this.#lifecycle)) {
      return;
    }
    this.#lifecycle = casework.Tag("Failed", {
      cause: "BackendFailed",
      detail,
    });
    this.#events.finish();
    this.#diagnostics.finish();
  }
}

type ParsedRawEvent =
  | Tagged<"Application", ApplicationEvent>
  | Tagged<"Diagnostic", DiagnosticEvent>
  | Tagged<"CommandSettled">
  | Tagged<"BackpressureExceeded", { readonly rejectedEventBytes: number }>
  | Tagged<"Stopped", { readonly cause: string }>
  | Tagged<"ContractViolation", { readonly detail: string }>;

type RawNativeEventType =
  | "singleDelivery"
  | "request"
  | "response"
  | "responseSegment"
  | "resourceReceived"
  | "resourceSegment"
  | "resourceNeedsDecompression"
  | "channelMessage"
  | "announce"
  | "linkEstablished"
  | "peerIdentified"
  | "linkClosed"
  | "linkInterfaceMismatch"
  | "resourceAssembled"
  | "resourceFailed"
  | "resourceSendProgress"
  | "selfRatchetRotated"
  | "announceHeldDropped"
  | "delivered"
  | "routeExpired"
  | "routeEvicted"
  | "routeInterfaceGone"
  | "routeDropped"
  | "commandSettled"
  | "eventBackpressureExceeded"
  | "nodeStopped"
  | "eventOverflow"
  | "message";

type RawNativeEvent = {
  [Name in RawNativeEventType]: Tagged<Name, Record<string, unknown>>;
}[RawNativeEventType];

const RAW_NATIVE_EVENT_TYPES: ReadonlySet<string> =
  new Set<RawNativeEventType>([
    "singleDelivery",
    "request",
    "response",
    "responseSegment",
    "resourceReceived",
    "resourceSegment",
    "resourceNeedsDecompression",
    "channelMessage",
    "announce",
    "linkEstablished",
    "peerIdentified",
    "linkClosed",
    "linkInterfaceMismatch",
    "resourceAssembled",
    "resourceFailed",
    "resourceSendProgress",
    "selfRatchetRotated",
    "announceHeldDropped",
    "delivered",
    "routeExpired",
    "routeEvicted",
    "routeInterfaceGone",
    "routeDropped",
    "commandSettled",
    "eventBackpressureExceeded",
    "nodeStopped",
    "eventOverflow",
    "message",
  ]);

function parseRawEvent(raw: unknown): ParsedRawEvent {
  const event = record("native event", raw);
  const type = text("native event type", event.type);
  if (!RAW_NATIVE_EVENT_TYPES.has(type)) {
    return casework.Tag("ContractViolation", {
      detail: `native backend emitted unknown event ${type}`,
    });
  }
  const tagged = casework.Tag(
    type as RawNativeEventType,
    event,
  ) as RawNativeEvent;
  return casework.match_into<ParsedRawEvent>().from(tagged, {
    singleDelivery: (data) =>
      casework.Tag(
        "Application",
        casework.Tag("SingleDelivery", {
          destination: contract.destinationHash(
            bytes("destination", data.destination),
          ),
          sourceInterface: contract.interfaceId(
            bytes("sourceInterface", data.sourceInterface),
          ),
          plaintext: bytes("plaintext", data.plaintext).slice(),
        }),
      ),
    request: (rawRequest) => {
      const request = {
        destination: contract.destinationHash(
          bytes("destination", rawRequest.destination),
        ),
        linkId: contract.linkId(bytes("linkId", rawRequest.linkId)),
        requestId: contract.requestId(
          bytes("requestId", rawRequest.requestId),
        ),
        pathHash: contract.requestPathHash(
          bytes("pathHash", rawRequest.pathHash),
        ),
        rttMillis: finiteNonNegative("rttMillis", rawRequest.rttMillis),
        data: bytes("data", rawRequest.data).slice(),
      };
      const requester = optionalBytes(rawRequest.requester);
      return casework.Tag(
        "Application",
        casework.Tag(
          "Request",
          requester
            ? { ...request, requester: contract.identityHash(requester) }
            : request,
        ),
      );
    },
    response: (data) =>
      casework.Tag(
        "Application",
        casework.Tag("Response", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          requestId: contract.requestId(bytes("requestId", data.requestId)),
          data: bytes("data", data.data).slice(),
        }),
      ),
    responseSegment: (data) =>
      casework.Tag(
        "Application",
        casework.Tag("ResponseSegment", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          requestId: contract.requestId(bytes("requestId", data.requestId)),
          segmentIndex: finiteNonNegative("segmentIndex", data.segmentIndex),
          totalSegments: finiteNonNegative("totalSegments", data.totalSegments),
          data: bytes("data", data.data).slice(),
        }),
      ),
    resourceReceived: (data) => {
      const details = {
        linkId: contract.linkId(bytes("linkId", data.linkId)),
        hash: contract.resourceHash(bytes("hash", data.hash)),
        resource: new resources.MemoryResourceStream(bytes("data", data.data)),
      };
      const metadata = optionalBytes(data.metadata);
      return casework.Tag(
        "Application",
        casework.Tag(
          "ResourceAvailable",
          metadata ? { ...details, metadata: metadata.slice() } : details,
        ),
      );
    },
    resourceSegment: (data) => {
      const details = {
        linkId: contract.linkId(bytes("linkId", data.linkId)),
        originalHash: contract.resourceHash(
          bytes("originalHash", data.originalHash),
        ),
        segmentIndex: finiteNonNegative("segmentIndex", data.segmentIndex),
        totalSegments: finiteNonNegative("totalSegments", data.totalSegments),
        data: bytes("data", data.data).slice(),
      };
      const metadata = optionalBytes(data.metadata);
      return casework.Tag(
        "Application",
        casework.Tag(
          "ResourceSegment",
          metadata ? { ...details, metadata: metadata.slice() } : details,
        ),
      );
    },
    resourceNeedsDecompression: (data) =>
      casework.Tag(
        "Application",
        casework.Tag("ResourceNeedsDecompression", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          hash: contract.resourceHash(bytes("hash", data.hash)),
          stream: bytes("stream", data.stream).slice(),
          uncompressedDataBytes: finiteNonNegative(
            "uncompressedDataBytes",
            data.uncompressedDataBytes,
          ),
        }),
      ),
    channelMessage: (data) =>
      casework.Tag(
        "Application",
        casework.Tag("ChannelMessage", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          messageType: text("messageType", data.messageType),
          data: bytes("data", data.data).slice(),
        }),
      ),
    announce: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("AnnounceHeard", {
          destination: contract.destinationHash(
            bytes("destination", data.destination),
          ),
          hops: finiteNonNegative("hops", data.hops),
          sourceInterface: contract.interfaceId(
            bytes("sourceInterface", data.sourceInterface),
          ),
        }),
      ),
    linkEstablished: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("LinkEstablished", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          rttMillis: finiteNonNegative("rttMillis", data.rttMillis),
        }),
      ),
    peerIdentified: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("PeerIdentified", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          identity: contract.identityHash(bytes("identity", data.identity)),
        }),
      ),
    linkClosed: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("LinkClosed", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          reason: linkClosedReason(data.reason),
        }),
      ),
    linkInterfaceMismatch: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("LinkInterfaceMismatch", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          attachedInterface: contract.interfaceId(
            bytes("attachedInterface", data.attachedInterface),
          ),
          arrivedOn: contract.interfaceId(bytes("arrivedOn", data.arrivedOn)),
        }),
      ),
    resourceAssembled: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("ResourceAssembled", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          originalHash: contract.resourceHash(
            bytes("originalHash", data.originalHash),
          ),
          totalSizeBytes: finiteNonNegative(
            "totalSizeBytes",
            data.totalSizeBytes,
          ),
        }),
      ),
    resourceFailed: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("ResourceFailed", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          hash: contract.resourceHash(bytes("hash", data.hash)),
          cause: text("cause", data.cause),
        }),
      ),
    resourceSendProgress: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("ResourceSendProgress", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
          transferredBytes: finiteNonNegative(
            "transferredBytes",
            data.transferredBytes,
          ),
          totalBytes: finiteNonNegative("totalBytes", data.totalBytes),
          physicalTransferredBytes: finiteNonNegative(
            "physicalTransferredBytes",
            data.physicalTransferredBytes,
          ),
          segmentIndex: finiteNonNegative("segmentIndex", data.segmentIndex),
          totalSegments: finiteNonNegative(
            "totalSegments",
            data.totalSegments,
          ),
        }),
      ),
    selfRatchetRotated: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("SelfRatchetRotated", {
          destination: contract.destinationHash(
            bytes("destination", data.destination),
          ),
        }),
      ),
    announceHeldDropped: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("AnnounceHeldDropped", {
          destination: contract.destinationHash(
            bytes("destination", data.destination),
          ),
          sourceInterface: contract.interfaceId(
            bytes("sourceInterface", data.sourceInterface),
          ),
          cause: text("cause", data.cause),
        }),
      ),
    delivered: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("Delivered", {
          detail: text("detail", data.detail),
        }),
      ),
    routeExpired: (data) => routeDiagnostic("RouteExpired", data),
    routeEvicted: (data) => routeDiagnostic("RouteEvicted", data),
    routeInterfaceGone: (data) =>
      routeDiagnostic("RouteInterfaceGone", data),
    routeDropped: (data) => routeDiagnostic("RouteDropped", data),
    commandSettled: () => casework.Tag("CommandSettled"),
    eventBackpressureExceeded: (data) =>
      casework.Tag("BackpressureExceeded", {
        rejectedEventBytes: finiteNonNegative(
          "rejectedEventBytes",
          data.rejectedEventBytes,
        ),
      }),
    nodeStopped: (data) =>
      casework.Tag("Stopped", {
        cause: text("cause", data.cause),
      }),
    eventOverflow: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("DiagnosticsDropped", {
          count: BigInt(
            finiteNonNegative(
              "droppedDiagnostics",
              data.droppedDiagnostics,
            ),
          ),
        }),
      ),
    message: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("BackendDiagnostic", {
          kind: "message",
          detail: stringify(data),
        }),
      ),
  });
}

type RouteDiagnosticName =
  | "RouteExpired"
  | "RouteEvicted"
  | "RouteInterfaceGone"
  | "RouteDropped";

function routeDiagnostic(
  name: RouteDiagnosticName,
  event: Record<string, unknown>,
): ParsedRawEvent {
  return casework.Tag(
    "Diagnostic",
    casework.Tag(name, {
      destination: contract.destinationHash(
        bytes("destination", event.destination),
      ),
    }),
  );
}

function validateCreateOptions(options: PrnsCreateOptions): {
  readonly raw: RawNodeOptions;
  readonly limits: PrnsLimits;
} {
  const limits = validateLimits(options.limits ?? contract.balancedLimits());
  const raw: RawNodeOptions = {
    eventQueueLimit: limits.applicationEvents + limits.diagnostics,
    applicationEventQueueLimit: limits.applicationEvents,
    retainedEventBytesLimit: limits.retainedEventBytes,
    diagnosticEventQueueLimit: limits.diagnostics,
  };
  const identity = rawIdentity(options.identity);
  if (identity !== undefined) {
    raw.identity = identity;
  }
  if (options.transport !== undefined) {
    raw.transport = options.transport;
  }
  if (options.destinations !== undefined) {
    raw.destinations = options.destinations.map(rawDestination);
  }
  return { raw, limits };
}

function rawIdentity(identity: IdentityConfig): RawIdentity | undefined {
  return casework.match(identity, {
    Existing: ({ secret }) => ({ secret: Buffer.from(secret) }),
    GenerateEphemeral: () => undefined,
    LoadOrCreate: ({ path }) => ({
      path: nonEmpty("identity path", path),
    }),
  });
}

function rawDestination(destination: DestinationConfig): RawDestination {
  const name = destination.data.name;
  const appName = nonEmpty("destination appName", name.appName);
  if (name.aspects.length === 0) {
    throw new contract.PrnsValidationError(
      "MissingDestinationAspect",
      "destination aspects must contain at least one component",
    );
  }
  const aspects = name.aspects.map((aspect) =>
    nonEmpty("destination aspect", aspect),
  );
  return casework.match(destination, {
    Plain: (): RawDestination => ({ appName, aspects, kind: "plain" }),
    Single: ({ identity, announceAppData }): RawDestination => {
      const raw: RawDestination = { appName, aspects, kind: "single" };
      if (identity !== undefined) {
        const configuredIdentity = rawIdentity(identity);
        if (configuredIdentity !== undefined) {
          raw.identity = configuredIdentity;
        }
      }
      if (announceAppData !== undefined) {
        raw.announceAppData = Buffer.from(
          bytes("announceAppData", announceAppData),
        );
      }
      return raw;
    },
  });
}

function validateLimits(limits: PrnsLimits): PrnsLimits {
  return {
    pendingCommands: positiveInteger("pendingCommands", limits.pendingCommands),
    applicationEvents: positiveInteger(
      "applicationEvents",
      limits.applicationEvents,
    ),
    retainedEventBytes: positiveInteger(
      "retainedEventBytes",
      limits.retainedEventBytes,
    ),
    diagnostics: positiveInteger("diagnostics", limits.diagnostics),
  };
}

function retainedEventBytes(event: ApplicationEvent): number {
  return casework.match_into<number>().from(event, {
    SingleDelivery: ({ plaintext }) => plaintext.length,
    Request: ({ data }) => data.length,
    Response: ({ data }) => data.length,
    ResponseSegment: ({ data }) => data.length,
    ResourceAvailable: ({ resource, metadata }) =>
      resource.totalBytes + (metadata?.length ?? 0),
    ResourceSegment: ({ data, metadata }) =>
      data.length + (metadata?.length ?? 0),
    ResourceNeedsDecompression: ({ stream }) => stream.length,
    ChannelMessage: ({ messageType, data }) =>
      messageType.length + data.length,
  });
}

function isStopped(state: LifecycleState): boolean {
  return state.tag === "Stopped" || state.tag === "Failed" || state.tag === "Stopping";
}

function operationFailed(operation: string, error: unknown): OperationFailed {
  const details = errorDetails(error);
  return casework.Tag(
    "OperationFailed",
    details.code === undefined
      ? { operation, detail: details.detail }
      : { operation, detail: details.detail, code: details.code },
  );
}

function backendStartFailed(error: unknown): BackendStartFailed {
  const details = errorDetails(error);
  return casework.Tag(
    "BackendStartFailed",
    details.code === undefined
      ? { detail: details.detail }
      : { detail: details.detail, code: details.code },
  );
}

function errorDetails(error: unknown): {
  readonly detail: string;
  readonly code?: string;
} {
  if (error instanceof Error) {
    const code =
      "code" in error && typeof error.code === "string"
        ? error.code
        : undefined;
    return code === undefined
      ? { detail: error.message }
      : { detail: error.message, code };
  }
  return { detail: String(error) };
}

function positiveInteger(name: string, value: number): number {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new contract.PrnsValidationError(
      "InvalidLimit",
      `${name} must be a positive safe integer`,
    );
  }
  return value;
}

function finiteNonNegative(name: string, value: unknown): number {
  if (
    typeof value !== "number" ||
    !Number.isFinite(value) ||
    value < 0
  ) {
    throw new contract.PrnsValidationError(
      "InvalidNumber",
      `${name} must be a finite non-negative number`,
    );
  }
  return value;
}

function optionalBitrate<Value extends object>(
  value: Value,
  bitrateBps: number | undefined,
): Value & { bitrateBps?: number } {
  return bitrateBps === undefined
    ? value
    : {
        ...value,
        bitrateBps: positiveInteger("bitrateBps", bitrateBps),
      };
}

function nonEmpty(name: string, value: string): string {
  if (value.length === 0) {
    throw new contract.PrnsValidationError(
      "EmptyString",
      `${name} must not be empty`,
    );
  }
  return value;
}

function bytes(name: string, value: unknown): Uint8Array {
  if (!(value instanceof Uint8Array)) {
    throw new contract.PrnsValidationError(
      "InvalidBytes",
      `${name} must be a Uint8Array`,
    );
  }
  return value;
}

function optionalBytes(value: unknown): Uint8Array | undefined {
  return value === undefined ? undefined : bytes("optional bytes", value);
}

function text(name: string, value: unknown): string {
  if (typeof value !== "string") {
    throw new contract.PrnsValidationError(
      "EmptyString",
      `${name} must be a string`,
    );
  }
  return value;
}

function record(name: string, value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new contract.PrnsValidationError(
      "InvalidNumber",
      `${name} must be an object`,
    );
  }
  return value as Record<string, unknown>;
}

type RawLinkClosedReason = "timeout" | "peerClosed" | "malformedRtt";

const RAW_LINK_CLOSED_REASONS: ReadonlySet<string> =
  new Set<RawLinkClosedReason>([
    "timeout",
    "peerClosed",
    "malformedRtt",
  ]);

function linkClosedReason(
  value: unknown,
): "Timeout" | "PeerClosed" | "MalformedRtt" {
  if (
    typeof value !== "string" ||
    !RAW_LINK_CLOSED_REASONS.has(value)
  ) {
    throw new contract.PrnsValidationError(
      "EmptyString",
      `unknown link close reason ${String(value)}`,
    );
  }
  return casework.match(value as RawLinkClosedReason, {
    timeout: () => "Timeout" as const,
    peerClosed: () => "PeerClosed" as const,
    malformedRtt: () => "MalformedRtt" as const,
  });
}

function stringify(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}
