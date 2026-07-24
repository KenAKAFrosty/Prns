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
  ResourceStream,
} from "../contract.js";
import type { Tag as Tagged } from "../casework.js";

type Buffer = Uint8Array;

declare const Buffer: {
  from(bytes: Uint8Array): Buffer;
};

declare function require(path: string): unknown;

const casework = require("../../dist-cjs/casework.js") as typeof import("../casework.js");
const contract = require("../../dist-cjs/contract.js") as typeof import("../contract.js");
const lanes = require("../../dist-cjs/async_lanes.js") as typeof import("../async_lanes.js");
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
  IDENTITY_SECRET_LENGTH,
  PrnsValidationError,
  balancedLimits,
  destinationHash,
  identityHash,
  interfaceId,
  linkId,
  requestId,
  requestPathHash,
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
  ResourceStream,
} from "../contract.js";
export type {
  DataFrom,
  Tag as Tagged,
  TagFrom,
} from "../casework.js";

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

export const PrnsConsumerError = lanes.PrnsConsumerError;

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

  events(): AsyncIterable<ApplicationEvent> {
    return this.#events;
  }

  diagnostics(): AsyncIterable<DiagnosticEvent> {
    return this.#diagnostics;
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
    if (parsed.tag === "Application") {
      this.#events.push(parsed.data);
      return;
    }
    if (parsed.tag === "Diagnostic") {
      this.#diagnostics.push(parsed.data);
      return;
    }
    if (parsed.tag === "Stopped") {
      if (!isStopped(this.#lifecycle)) {
        this.#lifecycle = casework.Tag("Stopped", { reason: "BackendExited" });
      }
      this.#events.finish();
      this.#diagnostics.finish();
      return;
    }
    if (parsed.tag === "CommandSettled") {
      return;
    }
    this.#failBackend(parsed.data.detail);
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

class MemoryResourceStream implements ResourceStream {
  readonly totalBytes: number;
  readonly #data: Uint8Array;
  #claimed = false;

  constructor(data: Uint8Array) {
    this.#data = data;
    this.totalBytes = data.length;
  }

  [Symbol.asyncIterator](): AsyncIterator<Uint8Array> {
    if (this.#claimed) {
      throw new PrnsConsumerError("Resource");
    }
    this.#claimed = true;
    let offset = 0;
    return {
      next: async () => {
        if (offset === this.#data.length) {
          return { done: true, value: undefined };
        }
        const end = Math.min(offset + 64 * 1_024, this.#data.length);
        const value = this.#data.slice(offset, end);
        offset = end;
        return { done: false, value };
      },
    };
  }
}

type ParsedRawEvent =
  | Tagged<"Application", ApplicationEvent>
  | Tagged<"Diagnostic", DiagnosticEvent>
  | Tagged<"CommandSettled">
  | Tagged<"Stopped">
  | Tagged<"ContractViolation", { readonly detail: string }>;

function parseRawEvent(raw: unknown): ParsedRawEvent {
  const event = record("native event", raw);
  const type = text("native event type", event.type);
  switch (type) {
    case "singleDelivery":
      return casework.Tag(
        "Application",
        casework.Tag("SingleDelivery", {
          destination: contract.destinationHash(bytes("destination", event.destination)),
          sourceInterface: contract.interfaceId(bytes("sourceInterface", event.sourceInterface)),
          plaintext: bytes("plaintext", event.plaintext).slice(),
        }),
      );
    case "request": {
      const data = {
        destination: contract.destinationHash(bytes("destination", event.destination)),
        linkId: contract.linkId(bytes("linkId", event.linkId)),
        requestId: contract.requestId(bytes("requestId", event.requestId)),
        pathHash: contract.requestPathHash(bytes("pathHash", event.pathHash)),
        rttMillis: finiteNonNegative("rttMillis", event.rttMillis),
        data: bytes("data", event.data).slice(),
        responseToken: event.token,
      };
      const requester = optionalBytes(event.requester);
      return casework.Tag(
        "Application",
        casework.Tag(
          "Request",
          requester
            ? { ...data, requester: contract.identityHash(requester) }
            : data,
        ),
      );
    }
    case "response":
      return casework.Tag(
        "Application",
        casework.Tag("Response", {
          linkId: contract.linkId(bytes("linkId", event.linkId)),
          requestId: contract.requestId(bytes("requestId", event.requestId)),
          data: bytes("data", event.data).slice(),
        }),
      );
    case "resourceReceived": {
      const data = bytes("data", event.data).slice();
      const details = {
        linkId: contract.linkId(bytes("linkId", event.linkId)),
        hash: bytes("hash", event.hash).slice(),
        resource: new MemoryResourceStream(data),
      };
      const metadata = optionalBytes(event.metadata);
      return casework.Tag(
        "Application",
        casework.Tag(
          "ResourceAvailable",
          metadata ? { ...details, metadata: metadata.slice() } : details,
        ),
      );
    }
    case "channelMessage":
      return casework.Tag(
        "Application",
        casework.Tag("ChannelMessage", {
          linkId: contract.linkId(bytes("linkId", event.linkId)),
          messageType: text("messageType", event.messageType),
          data: bytes("data", event.data).slice(),
        }),
      );
    case "announce":
      return casework.Tag(
        "Diagnostic",
        casework.Tag("AnnounceHeard", {
          destination: contract.destinationHash(bytes("destination", event.destination)),
          hops: finiteNonNegative("hops", event.hops),
          sourceInterface: contract.interfaceId(bytes("sourceInterface", event.sourceInterface)),
        }),
      );
    case "linkEstablished":
      return casework.Tag(
        "Diagnostic",
        casework.Tag("LinkEstablished", {
          linkId: contract.linkId(bytes("linkId", event.linkId)),
          rttMillis: finiteNonNegative("rttMillis", event.rttMillis),
        }),
      );
    case "peerIdentified":
      return casework.Tag(
        "Diagnostic",
        casework.Tag("PeerIdentified", {
          linkId: contract.linkId(bytes("linkId", event.linkId)),
          identity: contract.identityHash(bytes("identity", event.identity)),
        }),
      );
    case "linkClosed":
      return casework.Tag(
        "Diagnostic",
        casework.Tag("LinkClosed", {
          linkId: contract.linkId(bytes("linkId", event.linkId)),
          reason: linkClosedReason(event.reason),
        }),
      );
    case "commandSettled":
      return casework.Tag("CommandSettled");
    case "nodeStopped":
      return casework.Tag("Stopped");
    case "eventOverflow":
      return casework.Tag(
        "Diagnostic",
        casework.Tag("DiagnosticsDropped", {
          count: BigInt(finiteNonNegative("droppedDiagnostics", event.droppedDiagnostics)),
        }),
      );
    case "responseSegment":
    case "resourceSegment":
    case "selfRatchetRotated":
    case "announceHeldDropped":
    case "linkInterfaceMismatch":
    case "resourceAssembled":
    case "resourceFailed":
    case "resourceSendProgress":
    case "routeExpired":
    case "routeEvicted":
    case "routeInterfaceGone":
    case "routeDropped":
    case "delivered":
    case "message":
      return casework.Tag(
        "Diagnostic",
        casework.Tag("BackendDiagnostic", {
          kind: type,
          detail: stringify(event),
        }),
      );
    default:
      return casework.Tag("ContractViolation", {
        detail: `native backend emitted unknown event ${type}`,
      });
  }
}

function validateCreateOptions(options: PrnsCreateOptions): {
  readonly raw: RawNodeOptions;
  readonly limits: PrnsLimits;
} {
  const limits = validateLimits(options.limits ?? contract.balancedLimits());
  const raw: RawNodeOptions = {
    eventQueueLimit: limits.applicationEvents + limits.diagnostics,
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
  switch (identity.tag) {
    case "Existing":
      return { secret: Buffer.from(identity.data.secret) };
    case "GenerateEphemeral":
      return undefined;
    case "LoadOrCreate":
      return { path: nonEmpty("identity path", identity.data.path) };
  }
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
  if (destination.tag === "Plain") {
    return { appName, aspects, kind: "plain" };
  }
  const raw: RawDestination = { appName, aspects, kind: "single" };
  if (destination.data.identity !== undefined) {
    const identity = rawIdentity(destination.data.identity);
    if (identity !== undefined) {
      raw.identity = identity;
    }
  }
  if (destination.data.announceAppData !== undefined) {
    raw.announceAppData = Buffer.from(
      bytes("announceAppData", destination.data.announceAppData),
    );
  }
  return raw;
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
  switch (event.tag) {
    case "SingleDelivery":
      return event.data.plaintext.length;
    case "Request":
      return event.data.data.length;
    case "Response":
      return event.data.data.length;
    case "ResourceAvailable":
      return event.data.metadata?.length ?? 0;
    case "ChannelMessage":
      return event.data.messageType.length + event.data.data.length;
  }
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

function linkClosedReason(
  value: unknown,
): "Timeout" | "PeerClosed" | "MalformedRtt" {
  switch (value) {
    case "timeout":
      return "Timeout";
    case "peerClosed":
      return "PeerClosed";
    case "malformedRtt":
      return "MalformedRtt";
    default:
      throw new contract.PrnsValidationError(
        "EmptyString",
        `unknown link close reason ${String(value)}`,
      );
  }
}

function stringify(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}
