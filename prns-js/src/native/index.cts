import type {
  ApplicationEvent,
  BackendCapabilities,
  BackendInfo,
  BackendKind,
  BackendStartFailed,
  Bitrate,
  CapabilityName,
  CommandFailure,
  CommandOutcome,
  CommandSettlement,
  CommandSettlementFor,
  ContractMismatch,
  DeliveryEvidenceKind,
  DestinationConfig,
  DestinationHash,
  DiagnosticEvent,
  HostCommand,
  HostSnapshot,
  IdentityConfig,
  IdentityHash,
  InterfaceConfig,
  InterfaceId,
  InterfaceHealth,
  InterfaceKind,
  InterfaceRoutingPolicy,
  LifecycleState,
  LinkId,
  PrnsCreateOptions,
  PrnsLimits,
  PersistenceFlushCause,
  PersistenceFlushTarget,
  RequestId,
  RequestPathHash,
  ResourceCompression,
  ResourceStrategy,
  ResponseTimeout,
  WebSocketFramingSelection,
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
  HOST_SCHEMA_VERSION,
  PRODUCT_VERSION,
  DESTINATION_HASH_LENGTH,
  IDENTITY_HASH_LENGTH,
  INTERFACE_ID_LENGTH,
  LINK_ID_LENGTH,
  PACKET_HASH_LENGTH,
  REQUEST_ID_LENGTH,
  REQUEST_PATH_HASH_LENGTH,
  RESOURCE_HASH_LENGTH,
  IDENTITY_SECRET_LENGTH,
  SAFE_INT_MAX,
  SAFE_INT_MIN,
  PrnsValidationError,
  balancedLimits,
  destinationHash,
  identityHash,
  interfaceId,
  linkId,
  packetHash,
  requestId,
  requestPathHash,
  resourceHash,
  identitySecret,
} = contract;
export type {
  ApplicationEvent,
  BackendCapabilities,
  BackendInfo,
  BackendKind,
  BackendStartFailed,
  Bitrate,
  CapabilityName,
  CommandFailure,
  CommandOutcome,
  CommandSettlement,
  CommandSettlementFor,
  ContractMismatch,
  DeliveryEvidenceKind,
  DestinationConfig,
  DestinationHash,
  DestinationIdentityConfig,
  DestinationName,
  DiagnosticEvent,
  HostCommand,
  HostSnapshot,
  HostRoleName,
  IdentityConfig,
  IdentityHash,
  IdentitySecret,
  InterfaceConfig,
  InterfaceHealth,
  InterfaceId,
  InterfaceKind,
  LifecycleState,
  LinkId,
  PacketHash,
  PrnsCreateOptions,
  PrnsLimits,
  PersistenceConfig,
  PrnsValidationCode,
  RequestId,
  RequestHandlerConfig,
  RequestPolicy,
  RequestPathHash,
  ResourceCompression,
  ResourceHash,
  ResourceStrategy,
  ResourceStream,
  ResponseTimeout,
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
  useHostIdentity?: boolean;
  announceAppData?: Buffer;
  maximumRequestBytes?: number;
  requestPaths?: {
    path: string;
    policy: "allowNone" | "allowAll" | "allowList";
  }[];
};

type RawNodeOptions = {
  identity?: RawIdentity;
  role?: "endpoint" | "transport";
  destinations?: RawDestination[];
  eventQueueLimit?: number;
  applicationEventQueueLimit?: number;
  retainedEventBytesLimit?: number;
  diagnosticEventQueueLimit?: number;
  persistencePath?: string;
};

type RawSerialLine = {
  baud: number;
  dataBits: string;
  parity: string;
  stopBits: string;
};

type RawRNodeRadio = {
  frequencyHz: number;
  bandwidthHz: number;
  txPowerDbm: number;
  spreadingFactor: number;
  codingRate: number;
};

type RawMultiRNodeMember = {
  name: string;
  virtualPort: number;
  radio: RawRNodeRadio;
  flowControl: boolean;
  outgoing: boolean;
};

type RawInterfaceConfig = {
  kind: InterfaceKind;
  groupId?: string | undefined;
  discoveryScope?: string | undefined;
  discoveryPort?: number | undefined;
  dataPort?: number | undefined;
  devices?: string[];
  ignoredDevices?: string[];
  multicastAddressType?: string | undefined;
  target?: string;
  bind?: string;
  local?: string;
  peer?: string;
  bitrateBps?: number | undefined;
  port?: string;
  line?: RawSerialLine;
  flowControl?: boolean;
  preambleMillis?: number;
  transmitTailMillis?: number;
  persistence?: number;
  slotTimeMillis?: number;
  stationCallsign?: string | undefined;
  stationIntervalSeconds?: number | undefined;
  callsign?: string;
  ssid?: number;
  radio?: RawRNodeRadio;
  airtimeLimitShortCentiPercent?: number | undefined;
  airtimeLimitLongCentiPercent?: number | undefined;
  members?: RawMultiRNodeMember[];
  command?: string[];
  respawnDelayMillis?: number;
  peers?: string[];
  connectable?: boolean;
  url?: string;
  framing?: WebSocketFramingSelection;
};

type RawInterfaceRoutingPolicy = {
  mode?: string;
  gravity?: number;
  recursivePathRequests?: boolean;
  announcesFromInternal?: boolean;
  announcesToInternal?: boolean;
};

type RawNode = {
  readonly identityHash: Buffer;
  readonly destinationHashes: Buffer[];
  ready(): Promise<void>;
  stop(): Promise<void>;
  announce(
    destination: Buffer,
    options?: { interfaceId?: Buffer },
  ): Promise<void>;
  sendSinglePacket(
    destination: Buffer,
    data: Buffer,
  ): Promise<RawPacketReceipt>;
  establishLinkWithRtt(destination: Buffer): Promise<RawLinkInfo>;
  requestPath(destination: Buffer): Promise<RawPathInfo>;
  identify(linkId: Buffer, identity: Buffer): Promise<void>;
  sendLinkPacket(linkId: Buffer, data: Buffer): Promise<RawPacketReceipt>;
  request(
    linkId: Buffer,
    pathHash: Buffer,
    data: Buffer,
    options?: { timeoutMillis?: number; maximumResponseBytes?: number },
  ): Promise<RawRequestResult>;
  respond(token: RawRespondToken, data: Buffer): Promise<number>;
  sendResource(
    linkId: Buffer,
    data: Buffer,
    options: RawSendResourceOptions,
  ): Promise<void>;
  sendResourceFile(
    linkId: Buffer,
    path: string,
    options: RawSendResourceOptions,
  ): Promise<void>;
  setLinkResourceStrategy(
    linkId: Buffer,
    strategy: RawResourceStrategy,
  ): Promise<void>;
  setResourceStrategy(
    destination: Buffer,
    strategy: RawResourceStrategy,
  ): Promise<boolean>;
  sendChannelMessage(
    linkId: Buffer,
    messageType: number,
    data: Buffer,
  ): Promise<RawPacketReceipt>;
  allowRequester(
    destination: Buffer,
    pathHash: Buffer,
    identity: Buffer,
  ): Promise<void>;
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
  attachInterface(
    config: RawInterfaceConfig,
    routing?: RawInterfaceRoutingPolicy,
  ): Promise<RawInterface>;
  hostSnapshot(): Promise<RawHostSnapshot>;
};

type RawHostSnapshot = {
  revision: bigint;
  backend: {
    backend: string;
    capabilities: string[];
    interfaceKinds: string[];
  };
  interfaces: Array<{
    interfaceId: Buffer;
    name?: string;
    kind?: string;
    health: string;
    failureDetail?: string;
    rxBytes: bigint;
    txBytes: bigint;
    rxBps?: number;
    txBps?: number;
    routeCount: number;
    linkCount: number;
    transportedLinkCount: number;
  }>;
  routes: Array<{
    destination: Buffer;
    hops: number;
    viaIdentity?: Buffer;
    interfaceId: Buffer;
    learnedAtMillis: number;
    lastRouteActivityAtMillis: number;
    expiresAtMillis: number;
  }>;
  activeLinkCount: number;
  destinationIdentities: Array<{
    destination: Buffer;
    identity: Buffer;
  }>;
  runtime: HostSnapshot["runtime"];
  persistence: {
    persistent: boolean;
    restored: boolean;
    lastFlushCause?: string;
    lastFailureDetail?: string;
  };
};

type RawPacketReceipt = {
  rttMillis: number;
  evidence: string;
  packetHash?: Buffer;
};

type RawLinkInfo = {
  linkId: Buffer;
  rttMillis: number;
};

type RawPathInfo = {
  hops: number;
};

type RawRequestResult = {
  data: Buffer;
  packed: Buffer;
  rttMillis: number;
};

type RawRespondToken = {
  linkId: Buffer;
  requestId: Buffer;
  rttMillis: number;
};

type RawSendResourceOptions = {
  metadata?: Buffer;
  compression: "auto" | "never";
};

type RawResourceStrategy =
  | { accept: "none" }
  | {
      accept: "all";
      maxUncompressedBytes: number;
      acceptCompressed: boolean;
    };

type RawInterface = {
  readonly id: Buffer;
  readonly kind: string | null;
  teardown(): boolean;
};

type NativeBinding = {
  version(): string;
  hostContractAbi(): number;
  hostSchemaVersion(): number;
  backendInfo(): {
    backend: string;
    capabilities: string[];
    interfaceKinds: string[];
  };
  startNode(options: RawNodeOptions, onEvent: (event: unknown) => void): RawNode;
};

export type OperationFailed = Tagged<
  "OperationFailed",
  { readonly operation: string; readonly detail: string; readonly code?: string }
>;
export type StopOutcome =
  | Tagged<"Stopped">
  | Tagged<"AlreadyStopped">
  | OperationFailed;
type CommandCase<Name extends HostCommand["tag"]> = Extract<
  HostCommand,
  { readonly tag: Name }
>;
export type AnnounceOutcome = CommandSettlementFor<CommandCase<"Announce">>;
export type SendSinglePacketOutcome = CommandSettlementFor<
  CommandCase<"SendSinglePacket">
>;
export type CloseLinkOutcome = CommandSettlementFor<CommandCase<"CloseLink">>;
export type AttachOutcome = CommandSettlementFor<
  CommandCase<"AttachTcpServer" | "AttachTcpClient" | "AttachUdp" | "AttachInterface">
>;
export type DetachInterfaceOutcome = CommandSettlementFor<
  CommandCase<"DetachInterface">
>;
export type EstablishLinkOutcome = CommandSettlementFor<
  CommandCase<"EstablishLink">
>;
export type RequestPathOutcome = CommandSettlementFor<
  CommandCase<"RequestPath">
>;
export type IdentifyOutcome = CommandSettlementFor<CommandCase<"Identify">>;
export type SendLinkPacketOutcome = CommandSettlementFor<
  CommandCase<"SendLinkPacket">
>;
export type RequestOutcome = CommandSettlementFor<CommandCase<"Request">>;
export type RespondOutcome = CommandSettlementFor<CommandCase<"Respond">>;
export type SendResourceOutcome = CommandSettlementFor<
  CommandCase<"SendResource">
>;
export type SendResourceOptions = {
  readonly packedMetadata?: Uint8Array;
  readonly compression?: ResourceCompression;
};
export type SetResourceStrategyOutcome = CommandSettlementFor<
  CommandCase<"SetLinkResourceStrategy" | "SetDestinationResourceStrategy">
>;
export type SendChannelMessageOutcome = CommandSettlementFor<
  CommandCase<"SendChannelMessage">
>;
export type AllowRequesterOutcome = CommandSettlementFor<
  CommandCase<"AllowRequester">
>;
export type PrnsCreateOutcome =
  | Tagged<"Ready", Prns>
  | ContractMismatch
  | BackendStartFailed;

export function persistentEndpoint(
  root: string,
  destinations: readonly DestinationConfig[] = [],
): PrnsCreateOptions {
  const selected = nonEmpty("persistence root", root);
  const separator = selected.endsWith("/") || selected.endsWith("\\") ? "" : "/";
  return {
    identity: casework.Tag("LoadOrCreate", {
      path: `${selected}${separator}identity`,
    }),
    persistence: casework.Tag("Directory", {
      path: `${selected}${separator}state`,
    }),
    role: "Endpoint",
    destinations,
  };
}

const RAW_BACKEND_INFO = addon.backendInfo();
const NATIVE_BACKEND_INFO: BackendInfo = Object.freeze({
  backend: contract.contractValue("backend", RAW_BACKEND_INFO.backend, contract.isBackendKind),
  capabilities: Object.freeze(
    RAW_BACKEND_INFO.capabilities.map((value) =>
      contract.contractValue("capabilities", value, contract.isCapabilityName),
    ),
  ),
  interfaceKinds: Object.freeze(
    RAW_BACKEND_INFO.interfaceKinds.map((value) =>
      contract.contractValue("interfaceKinds", value, contract.isInterfaceKind),
    ),
  ),
});
const NATIVE_CAPABILITIES: ReadonlySet<CapabilityName> = new Set(
  NATIVE_BACKEND_INFO.capabilities,
);
const NATIVE_INTERFACE_KINDS: ReadonlySet<InterfaceKind> = new Set(
  NATIVE_BACKEND_INFO.interfaceKinds,
);

function decodeHostSnapshot(raw: RawHostSnapshot): HostSnapshot {
  const backend: BackendInfo = {
    backend: contract.contractValue("snapshot backend", raw.backend.backend, contract.isBackendKind),
    capabilities: raw.backend.capabilities.map((value) =>
      contract.contractValue("snapshot capabilities", value, contract.isCapabilityName),
    ),
    interfaceKinds: raw.backend.interfaceKinds.map((value) =>
      contract.contractValue("snapshot interface kinds", value, contract.isInterfaceKind),
    ),
  };
  return {
    revision: raw.revision,
    backend,
    interfaces: raw.interfaces.map((entry) => ({
      interfaceId: contract.interfaceId(entry.interfaceId),
      health: contract.contractValue("snapshot interface health", entry.health, contract.isInterfaceHealth),
      rxBytes: entry.rxBytes,
      txBytes: entry.txBytes,
      routeCount: entry.routeCount,
      linkCount: entry.linkCount,
      transportedLinkCount: entry.transportedLinkCount,
      ...(entry.name === undefined ? {} : { name: entry.name }),
      ...(entry.kind === undefined
        ? {}
        : {
            kind: contract.contractValue(
              "snapshot interface kind",
              entry.kind,
              contract.isInterfaceKind,
            ),
          }),
      ...(entry.failureDetail === undefined
        ? {}
        : { failureDetail: entry.failureDetail }),
      ...(entry.rxBps === undefined ? {} : { rxBps: entry.rxBps }),
      ...(entry.txBps === undefined ? {} : { txBps: entry.txBps }),
    })),
    routes: raw.routes.map((entry) => ({
      destination: contract.destinationHash(entry.destination),
      hops: entry.hops,
      interfaceId: contract.interfaceId(entry.interfaceId),
      learnedAtMillis: entry.learnedAtMillis,
      lastRouteActivityAtMillis: entry.lastRouteActivityAtMillis,
      expiresAtMillis: entry.expiresAtMillis,
      ...(entry.viaIdentity === undefined
        ? {}
        : { viaIdentity: contract.identityHash(entry.viaIdentity) }),
    })),
    activeLinkCount: raw.activeLinkCount,
    destinationIdentities: raw.destinationIdentities.map((entry) => ({
      destination: contract.destinationHash(entry.destination),
      identity: contract.identityHash(entry.identity),
    })),
    runtime: raw.runtime,
    persistence: {
      persistent: raw.persistence.persistent,
      restored: raw.persistence.restored,
      ...(raw.persistence.lastFlushCause === undefined
        ? {}
        : {
            lastFlushCause: raw.persistence
              .lastFlushCause as PersistenceFlushCause,
          }),
      ...(raw.persistence.lastFailureDetail === undefined
        ? {}
        : { lastFailureDetail: raw.persistence.lastFailureDetail }),
    },
  };
}

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
  readonly backendInfo: BackendInfo = NATIVE_BACKEND_INFO;
  readonly capabilities: BackendCapabilities = casework.Tag("Native", {
    available: NATIVE_CAPABILITIES,
    interfaceKinds: NATIVE_INTERFACE_KINDS,
  });
  readonly #limits: PrnsLimits;
  readonly #events: import("../async_lanes.js").BoundedAsyncLane<ApplicationEvent>;
  readonly #diagnostics: import("../async_lanes.js").BoundedAsyncLane<DiagnosticEvent>;
  readonly #raw: RawNode;
  readonly #interfaces = new Map<string, NativeInterface>();
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
    const actualSchemaVersion =
      typeof addon.hostSchemaVersion === "function"
        ? addon.hostSchemaVersion()
        : 0;
    const actualProductVersion = addon.version();
    if (
      actualAbi !== contract.HOST_CONTRACT_ABI ||
      actualSchemaVersion !== contract.HOST_SCHEMA_VERSION ||
      actualProductVersion !== contract.PRODUCT_VERSION
    ) {
      return Promise.resolve(
        casework.Tag("ContractMismatch", {
          requiredAbi: contract.HOST_CONTRACT_ABI,
          actualAbi,
          requiredSchemaVersion: contract.HOST_SCHEMA_VERSION,
          actualSchemaVersion,
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

  get identityHash(): IdentityHash {
    return contract.identityHash(this.#raw.identityHash);
  }

  get lifecycle(): LifecycleState {
    return this.#lifecycle;
  }

  async snapshot(): Promise<HostSnapshot> {
    return decodeHostSnapshot(await this.#raw.hostSnapshot());
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
      this.#interfaces.clear();
      this.#events.finish();
      this.#diagnostics.finish();
      return casework.Tag("Stopped");
    } catch (error) {
      const failure = operationFailed("stop", error);
      this.#failBackend(failure.data.detail);
      return failure;
    }
  }

  execute<Command extends HostCommand>(
    command: Command,
  ): Promise<CommandSettlementFor<Command>> {
    return this.#execute(command) as Promise<CommandSettlementFor<Command>>;
  }

  async #execute(command: HostCommand): Promise<CommandSettlement> {
    if (isStopped(this.#lifecycle)) {
      return commandFailed(casework.Tag("NodeStopped"));
    }
    if (this.#pendingCommands >= this.#limits.pendingCommands) {
      return commandFailed(casework.Tag("Busy"));
    }
    this.#pendingCommands += 1;
    try {
      const outcome = await casework.match_into<Promise<CommandOutcome>>().from(
        command,
        {
          Announce: async ({ destination, interface: interfaceId }) => {
            const options =
              interfaceId === undefined
                ? undefined
                : { interfaceId: Buffer.from(interfaceId) };
            await this.#raw.announce(Buffer.from(destination), options);
            return casework.Tag("Announced");
          },
          SendSinglePacket: async ({ destination, payload }) => {
            const receipt = await this.#raw.sendSinglePacket(
              Buffer.from(destination),
              Buffer.from(bytes("payload", payload)),
            );
            return packetDelivered(receipt);
          },
          CloseLink: async ({ linkId }) => {
            if (!this.#raw.closeLink(Buffer.from(linkId))) {
              throw new CommandRejected(casework.Tag("NodeStopped"));
            }
            return casework.Tag("LinkCloseQueued");
          },
          AttachTcpServer: async ({ bind, bitrate }) => {
            const attached = new NativeInterface(
              await this.#raw.attachTcpServer(
                optionalBitrate(
                  { bind: nonEmpty("bind", bind) },
                  bitrateBitsPerSecond(bitrate),
                ),
              ),
            );
            this.#interfaces.set(interfaceKey(attached.id), attached);
            return casework.Tag("InterfaceAttached", {
              interface: attached.id,
            });
          },
          AttachTcpClient: async ({ target, bitrate }) => {
            const attached = new NativeInterface(
              await this.#raw.attachTcpClient(
                optionalBitrate(
                  { target: nonEmpty("target", target) },
                  bitrateBitsPerSecond(bitrate),
                ),
              ),
            );
            this.#interfaces.set(interfaceKey(attached.id), attached);
            return casework.Tag("InterfaceAttached", {
              interface: attached.id,
            });
          },
          AttachUdp: async ({ local, peer, bitrate }) => {
            const attached = new NativeInterface(
              await this.#raw.attachUdp(
                optionalBitrate(
                  {
                    local: nonEmpty("local", local),
                    peer: nonEmpty("peer", peer),
                  },
                  bitrateBitsPerSecond(bitrate),
                ),
              ),
            );
            this.#interfaces.set(interfaceKey(attached.id), attached);
            return casework.Tag("InterfaceAttached", {
              interface: attached.id,
            });
          },
          AttachInterface: async ({ config, routing }) => {
            validateInterfaceConfig(config);
            const raw = await this.#raw.attachInterface(
              rawInterfaceConfig(config),
              rawInterfaceRoutingPolicy(routing),
            );
            const attached = new NativeInterface(raw);
            this.#interfaces.set(interfaceKey(attached.id), attached);
            return casework.Tag("InterfaceAttached", {
              interface: attached.id,
            });
          },
          DetachInterface: async ({ interface: interfaceId }) => {
            const key = interfaceKey(interfaceId);
            const attached = this.#interfaces.get(key);
            if (attached === undefined) {
              throw new CommandRejected(casework.Tag("UnknownInterface"));
            }
            attached.close();
            this.#interfaces.delete(key);
            return casework.Tag("InterfaceDetached", {
              interface: interfaceId,
            });
          },
          EstablishLink: async ({ destination }) => {
            const established = await this.#raw.establishLinkWithRtt(
              Buffer.from(destination),
            );
            return casework.Tag("LinkEstablished", {
              linkId: contract.linkId(established.linkId),
              rttMillis: rawSafeUint(
                "rttMillis",
                established.rttMillis,
              ),
            });
          },
          RequestPath: async ({ destination }) => {
            const path = await this.#raw.requestPath(
              Buffer.from(destination),
            );
            return casework.Tag("PathDiscovered", {
              hops: rawSafeUint("hops", path.hops),
            });
          },
          Identify: async ({ linkId, identity }) => {
            await this.#raw.identify(
              Buffer.from(linkId),
              Buffer.from(identity),
            );
            return casework.Tag("Identified");
          },
          SendLinkPacket: async ({ linkId, payload }) =>
            packetDelivered(
              await this.#raw.sendLinkPacket(
                Buffer.from(linkId),
                Buffer.from(bytes("payload", payload)),
              ),
            ),
          Request: async ({
            linkId,
            pathHash,
            payload,
            timeout,
            maximumResponseBytes,
          }) => {
            const response = await this.#raw.request(
              Buffer.from(linkId),
              Buffer.from(pathHash),
              Buffer.from(bytes("payload", payload)),
              rawRequestOptions(timeout, maximumResponseBytes),
            );
            return casework.Tag("ResponseReceived", {
              data: bytes("response data", response.data).slice(),
              rttMillis: rawSafeUint(
                "rttMillis",
                response.rttMillis,
              ),
            });
          },
          Respond: async ({
            linkId,
            requestId,
            requestRttMillis,
            payload,
          }) => {
            const rttMillis = await this.#raw.respond(
              {
                linkId: Buffer.from(linkId),
                requestId: Buffer.from(requestId),
                rttMillis: rawSafeUint(
                  "requestRttMillis",
                  requestRttMillis,
                ),
              },
              Buffer.from(bytes("payload", payload)),
            );
            return casework.Tag("ResponseSent", {
              rttMillis: rawSafeUint("rttMillis", rttMillis),
            });
          },
          SendResource: async ({
            linkId,
            payload,
            packedMetadata,
            compression,
          }) => {
            const options: RawSendResourceOptions = {
              compression: rawResourceCompression(compression),
            };
            if (packedMetadata !== undefined) {
              options.metadata = Buffer.from(
                bytes("packedMetadata", packedMetadata),
              );
            }
            await this.#raw.sendResource(
              Buffer.from(linkId),
              Buffer.from(bytes("payload", payload)),
              options,
            );
            return casework.Tag("ResourceSent");
          },
          SetLinkResourceStrategy: async ({ linkId, strategy }) => {
            await this.#raw.setLinkResourceStrategy(
              Buffer.from(linkId),
              rawResourceStrategy(strategy),
            );
            return casework.Tag("ResourceStrategySet");
          },
          SetDestinationResourceStrategy: async ({
            destination,
            strategy,
          }) => {
            const configured = await this.#raw.setResourceStrategy(
              Buffer.from(destination),
              rawResourceStrategy(strategy),
            );
            if (!configured) {
              throw new CommandRejected(
                casework.Tag("UnknownDestination"),
              );
            }
            return casework.Tag("ResourceStrategySet");
          },
          SendChannelMessage: async ({
            linkId,
            messageType,
            payload,
          }) => {
            if (
              !Number.isSafeInteger(messageType) ||
              messageType < 0 ||
              messageType > 0xefff
            ) {
              throw new CommandRejected(
                casework.Tag("InvalidChannelMessageType"),
              );
            }
            return packetDelivered(
              await this.#raw.sendChannelMessage(
                Buffer.from(linkId),
                messageType,
                Buffer.from(bytes("payload", payload)),
              ),
            );
          },
          AllowRequester: async ({
            destination,
            pathHash,
            identity,
          }) => {
            await this.#raw.allowRequester(
              Buffer.from(destination),
              Buffer.from(pathHash),
              Buffer.from(identity),
            );
            return casework.Tag("RequesterAllowed");
          },
        },
      );
      return casework.Tag("Succeeded", outcome);
    } catch (error) {
      return commandFailed(commandFailure(error)) as SendResourceOutcome;
    } finally {
      this.#pendingCommands -= 1;
    }
  }

  announce(
    destination: DestinationHash,
    interfaceId?: InterfaceId,
  ): Promise<AnnounceOutcome> {
    return this.execute(
      casework.Tag(
        "Announce",
        interfaceId === undefined
          ? { destination }
          : { destination, interface: interfaceId },
      ),
    );
  }

  sendSinglePacket(
    destination: DestinationHash,
    payload: Uint8Array,
  ): Promise<SendSinglePacketOutcome> {
    return this.execute(
      casework.Tag("SendSinglePacket", { destination, payload }),
    );
  }

  closeLink(linkId: LinkId): Promise<CloseLinkOutcome> {
    return this.execute(casework.Tag("CloseLink", { linkId }));
  }

  attachTcpServer(options: {
    readonly bind: string;
    readonly bitrateBps?: number;
  }): Promise<AttachOutcome> {
    return this.execute(
      casework.Tag("AttachTcpServer", {
        bind: options.bind,
        bitrate: commandBitrate(options.bitrateBps),
      }),
    );
  }

  attachTcpClient(options: {
    readonly target: string;
    readonly bitrateBps?: number;
  }): Promise<AttachOutcome> {
    return this.execute(
      casework.Tag("AttachTcpClient", {
        target: options.target,
        bitrate: commandBitrate(options.bitrateBps),
      }),
    );
  }

  attachUdp(options: {
    readonly local: string;
    readonly peer: string;
    readonly bitrateBps?: number;
  }): Promise<AttachOutcome> {
    return this.execute(
      casework.Tag("AttachUdp", {
        local: options.local,
        peer: options.peer,
        bitrate: commandBitrate(options.bitrateBps),
      }),
    );
  }

  attachInterface(
    config: InterfaceConfig,
    routing?: InterfaceRoutingPolicy,
  ): Promise<AttachOutcome> {
    return this.execute(
      routing === undefined
        ? casework.Tag("AttachInterface", { config })
        : casework.Tag("AttachInterface", { config, routing }),
    );
  }

  detachInterface(interfaceId: InterfaceId): Promise<DetachInterfaceOutcome> {
    return this.execute(
      casework.Tag("DetachInterface", { interface: interfaceId }),
    );
  }

  establishLink(
    destination: DestinationHash,
  ): Promise<EstablishLinkOutcome> {
    return this.execute(casework.Tag("EstablishLink", { destination }));
  }

  requestPath(destination: DestinationHash): Promise<RequestPathOutcome> {
    return this.execute(casework.Tag("RequestPath", { destination }));
  }

  identify(
    linkId: LinkId,
    identity: IdentityHash,
  ): Promise<IdentifyOutcome> {
    return this.execute(casework.Tag("Identify", { linkId, identity }));
  }

  sendLinkPacket(
    linkId: LinkId,
    payload: Uint8Array,
  ): Promise<SendLinkPacketOutcome> {
    return this.execute(
      casework.Tag("SendLinkPacket", { linkId, payload }),
    );
  }

  request(
    linkId: LinkId,
    pathHash: RequestPathHash,
    payload: Uint8Array,
    timeout: ResponseTimeout = casework.Tag("LinkDefault"),
    maximumResponseBytes?: number,
  ): Promise<RequestOutcome> {
    return this.execute(
      casework.Tag("Request", {
        linkId,
        pathHash,
        payload,
        timeout,
        ...(maximumResponseBytes === undefined
          ? {}
          : { maximumResponseBytes }),
      }),
    );
  }

  respond(
    linkId: LinkId,
    requestId: RequestId,
    requestRttMillis: number,
    payload: Uint8Array,
  ): Promise<RespondOutcome> {
    return this.execute(
      casework.Tag("Respond", {
        linkId,
        requestId,
        requestRttMillis,
        payload,
      }),
    );
  }

  sendResource(
    linkId: LinkId,
    payload: Uint8Array,
    options: SendResourceOptions = {},
  ): Promise<SendResourceOutcome> {
    return this.execute(
      casework.Tag("SendResource", {
        linkId,
        payload,
        compression: options.compression ?? casework.Tag("Auto"),
        ...(options.packedMetadata === undefined
          ? {}
          : { packedMetadata: options.packedMetadata }),
      }),
    );
  }

  async sendResourceFile(
    linkId: LinkId,
    path: string,
    options: SendResourceOptions = {},
  ): Promise<SendResourceOutcome> {
    if (isStopped(this.#lifecycle)) {
      return commandFailed(casework.Tag("NodeStopped")) as SendResourceOutcome;
    }
    if (this.#pendingCommands >= this.#limits.pendingCommands) {
      return commandFailed(casework.Tag("Busy")) as SendResourceOutcome;
    }
    this.#pendingCommands += 1;
    try {
      const rawOptions: RawSendResourceOptions = {
        compression: rawResourceCompression(
          options.compression ?? casework.Tag("Auto"),
        ),
      };
      if (options.packedMetadata !== undefined) {
        rawOptions.metadata = Buffer.from(
          bytes("packedMetadata", options.packedMetadata),
        );
      }
      await this.#raw.sendResourceFile(
        Buffer.from(linkId),
        nonEmpty("resource path", path),
        rawOptions,
      );
      return casework.Tag("Succeeded", casework.Tag("ResourceSent"));
    } catch (error) {
      return commandFailed(commandFailure(error)) as SendResourceOutcome;
    } finally {
      this.#pendingCommands -= 1;
    }
  }

  setLinkResourceStrategy(
    linkId: LinkId,
    strategy: ResourceStrategy,
  ): Promise<SetResourceStrategyOutcome> {
    return this.execute(
      casework.Tag("SetLinkResourceStrategy", { linkId, strategy }),
    );
  }

  setDestinationResourceStrategy(
    destination: DestinationHash,
    strategy: ResourceStrategy,
  ): Promise<SetResourceStrategyOutcome> {
    return this.execute(
      casework.Tag("SetDestinationResourceStrategy", {
        destination,
        strategy,
      }),
    );
  }

  sendChannelMessage(
    linkId: LinkId,
    messageType: number,
    payload: Uint8Array,
  ): Promise<SendChannelMessageOutcome> {
    return this.execute(
      casework.Tag("SendChannelMessage", {
        linkId,
        messageType,
        payload,
      }),
    );
  }

  allowRequester(
    destination: DestinationHash,
    pathHash: RequestPathHash,
    identity: IdentityHash,
  ): Promise<AllowRequesterOutcome> {
    return this.execute(
      casework.Tag("AllowRequester", {
        destination,
        pathHash,
        identity,
      }),
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
  | "linkDelivery"
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
  | "persistenceRestored"
  | "persistenceFlushed"
  | "persistenceFlushFailed"
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
    "linkDelivery",
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
    "persistenceRestored",
    "persistenceFlushed",
    "persistenceFlushFailed",
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
    linkDelivery: (data) =>
      casework.Tag(
        "Application",
        casework.Tag("LinkDelivery", {
          linkId: contract.linkId(bytes("linkId", data.linkId)),
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
        rttMillis: rawSafeUint("rttMillis", rawRequest.rttMillis),
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
          segmentIndex: rawSafeUint("segmentIndex", data.segmentIndex),
          totalSegments: rawSafeUint("totalSegments", data.totalSegments),
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
        segmentIndex: rawSafeUint("segmentIndex", data.segmentIndex),
        totalSegments: rawSafeUint("totalSegments", data.totalSegments),
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
          uncompressedDataBytes: nonNegativeBigInt(
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
          messageType: rawSafeUint("messageType", data.messageType),
          data: bytes("data", data.data).slice(),
        }),
      ),
    announce: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("AnnounceHeard", {
          appData: bytes("appData", data.appData).slice(),
          destination: contract.destinationHash(
            bytes("destination", data.destination),
          ),
          hops: rawSafeUint("hops", data.hops),
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
          rttMillis: rawSafeUint("rttMillis", data.rttMillis),
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
          totalSizeBytes: nonNegativeBigInt(
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
          transferredBytes: nonNegativeBigInt(
            "transferredBytes",
            data.transferredBytes,
          ),
          totalBytes: nonNegativeBigInt("totalBytes", data.totalBytes),
          physicalTransferredBytes: nonNegativeBigInt(
            "physicalTransferredBytes",
            data.physicalTransferredBytes,
          ),
          segmentIndex: rawSafeUint("segmentIndex", data.segmentIndex),
          totalSegments: rawSafeUint(
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
    persistenceRestored: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("PersistenceRestored", {
          routes: rawSafeUint("routes", data.routes),
          destinationIdentities: rawSafeUint(
            "destinationIdentities",
            data.destinationIdentities,
          ),
          tunnels: rawSafeUint("tunnels", data.tunnels),
          ratchets: rawSafeUint("ratchets", data.ratchets),
          refused: rawSafeUint("refused", data.refused),
          dropped: rawSafeUint("dropped", data.dropped),
        }),
      ),
    persistenceFlushed: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("PersistenceFlushed", {
          cause: persistenceCause(data.cause),
          target: persistenceTarget(data.target),
        }),
      ),
    persistenceFlushFailed: (data) =>
      casework.Tag(
        "Diagnostic",
        casework.Tag("PersistenceFlushFailed", {
          cause: persistenceCause(data.cause),
          target: persistenceTarget(data.target),
        }),
      ),
    commandSettled: () => casework.Tag("CommandSettled"),
    eventBackpressureExceeded: (data) =>
      casework.Tag("BackpressureExceeded", {
        rejectedEventBytes: rawSafeUint(
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
            rawSafeUint(
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

function persistenceCause(value: unknown): PersistenceFlushCause {
  const cause = text("persistence cause", value);
  switch (cause) {
    case "startup":
      return "Startup";
    case "interval":
      return "Interval";
    case "routeChange":
      return "RouteChange";
    case "ratchetRotation":
      return "RatchetRotation";
    case "shutdown":
      return "Shutdown";
  }
  throw new contract.PrnsValidationError(
    "InvalidEnum",
    `unknown persistence cause ${cause}`,
  );
}

function persistenceTarget(value: unknown): PersistenceFlushTarget {
  const target = text("persistence target", value);
  if (target === "routingState") {
    return "RoutingState";
  }
  if (target === "ratchets") {
    return "Ratchets";
  }
  throw new contract.PrnsValidationError(
    "InvalidEnum",
    `unknown persistence target ${target}`,
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
  raw.role = casework.match(options.role, {
    Endpoint: () => "endpoint" as const,
    Transport: () => "transport" as const,
  });
  if (options.destinations !== undefined) {
    raw.destinations = options.destinations.map(rawDestination);
  }
  if (options.persistence !== undefined) {
    casework.match(options.persistence, {
      Ephemeral: () => undefined,
      Directory: ({ path }) => {
        raw.persistencePath = nonEmpty("persistence path", path);
      },
    });
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
    Single: ({
      identity,
      announceAppData,
      maximumRequestBytes,
      requestHandlers,
    }): RawDestination => {
      const raw: RawDestination = { appName, aspects, kind: "single" };
      casework.match(identity, {
        HostIdentity: () => {
          raw.useHostIdentity = true;
        },
        DedicatedIdentity: ({ identity: dedicated }) => {
          const configuredIdentity = rawIdentity(dedicated);
          if (configuredIdentity !== undefined) {
            raw.identity = configuredIdentity;
          }
        },
      });
      if (announceAppData !== undefined) {
        raw.announceAppData = Buffer.from(
          bytes("announceAppData", announceAppData),
        );
      }
      if (maximumRequestBytes !== undefined) {
        raw.maximumRequestBytes = nonNegativeInteger(
          "maximumRequestBytes",
          maximumRequestBytes,
        );
      }
      raw.requestPaths = requestHandlers.map((handler) => ({
        path: nonEmpty("request handler path", handler.path),
        policy: casework.match(handler.policy, {
          AllowNone: () => "allowNone" as const,
          AllowAll: () => "allowAll" as const,
          AllowList: () => "allowList" as const,
        }),
      }));
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
    LinkDelivery: ({ plaintext }) => plaintext.length,
    Request: ({ data }) => data.length,
    Response: ({ data }) => data.length,
    ResponseSegment: ({ data }) => data.length,
    ResourceAvailable: ({ resource, metadata }) =>
      exactBytesAsSafeNumber("resource.totalBytes", resource.totalBytes) +
      (metadata?.length ?? 0),
    ResourceSegment: ({ data, metadata }) =>
      data.length + (metadata?.length ?? 0),
    ResourceNeedsDecompression: ({ stream }) => stream.length,
    ChannelMessage: ({ data }) => data.length,
  });
}

function isStopped(state: LifecycleState): boolean {
  return state.tag === "Stopped" || state.tag === "Failed" || state.tag === "Stopping";
}

class CommandRejected {
  readonly failure: CommandFailure;

  constructor(failure: CommandFailure) {
    this.failure = failure;
  }
}

function commandFailed(failure: CommandFailure): CommandSettlement {
  return casework.Tag("Failed", failure);
}

function commandFailure(error: unknown): CommandFailure {
  if (error instanceof CommandRejected) {
    return error.failure;
  }
  if (error instanceof contract.PrnsValidationError) {
    return casework.Tag("InvalidConfiguration", { detail: error.message });
  }
  const details = errorDetails(error);
  if (details.code === "PRNS_NODE_STOPPED") {
    return casework.Tag("NodeStopped");
  }
  if (details.code === "PRNS_BUSY") {
    return casework.Tag("Busy");
  }
  if (details.code === "PRNS_PAYLOAD_TOO_LARGE") {
    return casework.Tag("PayloadTooLarge");
  }
  if (details.code === "PRNS_RESPONSE_TOO_LARGE") {
    return casework.Tag("ResponseTooLarge");
  }
  if (details.code === "PRNS_NO_ROUTE_TO_DESTINATION") {
    return casework.Tag("NoRouteToDestination");
  }
  if (details.code === "PRNS_NOT_DIRECTLY_REACHABLE") {
    return casework.Tag("NotDirectlyReachable");
  }
  if (details.code === "PRNS_PACKET_CULLED") {
    return casework.Tag("PacketCulled");
  }
  if (
    details.code === "PRNS_DELIVERY_TIMED_OUT" ||
    details.code === "PRNS_LINK_TIMEOUT"
  ) {
    return casework.Tag("DeliveryTimedOut");
  }
  if (details.code === "PRNS_UNKNOWN_LINK") {
    return casework.Tag("UnknownLink");
  }
  if (details.code === "PRNS_LINK_NOT_ACTIVE") {
    return casework.Tag("LinkNotActive");
  }
  if (details.code === "PRNS_ENTROPY_UNAVAILABLE") {
    return casework.Tag("EntropyUnavailable");
  }
  if (details.code === "PRNS_NOT_LINK_INITIATOR") {
    return casework.Tag("NotLinkInitiator");
  }
  if (details.code === "PRNS_IDENTITY_NOT_HELD") {
    return casework.Tag("IdentityNotHeld");
  }
  if (details.code === "PRNS_UNKNOWN_REQUEST_HANDLER") {
    return casework.Tag("UnknownRequestHandler");
  }
  if (details.code === "PRNS_REQUEST_POLICY_NOT_ALLOW_LIST") {
    return casework.Tag("RequestPolicyNotAllowList");
  }
  if (details.code === "PRNS_REQUEST_ALLOW_LIST_FULL") {
    return casework.Tag("RequestAllowListFull");
  }
  if (details.code === "PRNS_LINK_BUSY") {
    return casework.Tag("LinkBusy");
  }
  if (details.code === "PRNS_RESOURCE_TABLE_FULL") {
    return casework.Tag("ResourceTableFull");
  }
  if (details.code === "PRNS_RESOURCE_METADATA_TOO_LARGE") {
    return casework.Tag("ResourceMetadataTooLarge");
  }
  if (details.code === "PRNS_RESOURCE_REJECTED_BY_PEER") {
    return casework.Tag("ResourceRejectedByPeer");
  }
  if (details.code === "PRNS_RESOURCE_SEQUENCING_FAILED") {
    return casework.Tag("ResourceSequencingFailed");
  }
  if (details.code === "PRNS_RESOURCE_PREDECESSOR_FAILED") {
    return casework.Tag("ResourcePredecessorFailed");
  }
  if (details.code === "PRNS_CHANNEL_WINDOW_FULL") {
    return casework.Tag("ChannelWindowFull");
  }
  if (details.code === "PRNS_CHANNEL_UNTRACKABLE") {
    return casework.Tag("ChannelUntrackable");
  }
  if (details.code === "PRNS_INVALID_CHANNEL_MESSAGE_TYPE") {
    return casework.Tag("InvalidChannelMessageType");
  }
  if (
    details.code === "PRNS_CONFIG_INVALID" ||
    details.code === "PRNS_INVALID_ARGUMENT"
  ) {
    return casework.Tag("InvalidConfiguration", { detail: details.detail });
  }
  if (
    details.code === "PRNS_BIND_FAILED" ||
    details.code === "PRNS_ATTACH_FAILED"
  ) {
    return casework.Tag("BindFailed", { detail: details.detail });
  }
  if (details.code === "PRNS_UNSUPPORTED") {
    return casework.Tag("UnsupportedByBackend");
  }
  if (details.code === "PRNS_UNKNOWN_INTERFACE") {
    return casework.Tag("UnknownInterface");
  }
  if (details.code === "PRNS_PERMISSION_DENIED") {
    return casework.Tag("PermissionDenied", { detail: details.detail });
  }
  if (
    details.code === "PRNS_DEVICE_UNAVAILABLE" ||
    details.code === "PRNS_UNAVAILABLE"
  ) {
    return casework.Tag("DeviceUnavailable", { detail: details.detail });
  }
  if (details.code === "PRNS_CONNECT_FAILED") {
    return casework.Tag("ConnectFailed", { detail: details.detail });
  }
  if (details.code === "PRNS_BACKEND_FAILED") {
    return casework.Tag("BackendFailed", { detail: details.detail });
  }
  return casework.Tag("WriteFailed", { detail: details.detail });
}

function packetDelivered(receipt: RawPacketReceipt): CommandOutcome {
  const delivered = {
    rttMillis: rawSafeUint("rttMillis", receipt.rttMillis),
    evidence: deliveryEvidence(receipt.evidence),
  };
  return casework.Tag(
    "PacketDelivered",
    receipt.packetHash === undefined
      ? delivered
      : {
          ...delivered,
          packetHash: contract.packetHash(receipt.packetHash),
        },
  );
}

function rawResponseTimeout(
  timeout: ResponseTimeout,
): { timeoutMillis: number } | undefined {
  return casework.match(timeout, {
    LinkDefault: () => undefined,
    Exact: ({ millis }) => ({
      timeoutMillis: nonNegativeInteger("timeout millis", millis),
    }),
  });
}

function rawRequestOptions(
  timeout: ResponseTimeout,
  maximumResponseBytes: number | undefined,
): { timeoutMillis?: number; maximumResponseBytes?: number } | undefined {
  const options: {
    timeoutMillis?: number;
    maximumResponseBytes?: number;
  } = rawResponseTimeout(timeout) ?? {};
  if (maximumResponseBytes !== undefined) {
    options.maximumResponseBytes = nonNegativeInteger(
      "maximumResponseBytes",
      maximumResponseBytes,
    );
  }
  return Object.keys(options).length === 0 ? undefined : options;
}

function rawResourceCompression(
  compression: ResourceCompression,
): "auto" | "never" {
  return casework.match(compression, {
    Auto: () => "auto" as const,
    Never: () => "never" as const,
  });
}

function rawResourceStrategy(
  strategy: ResourceStrategy,
): RawResourceStrategy {
  return casework.match(strategy, {
    Refuse: () => ({ accept: "none" as const }),
    Accept: ({
      maximumUncompressedBytes,
      acceptCompressed,
    }) => ({
      accept: "all" as const,
      maxUncompressedBytes: nonNegativeInteger(
        "maximumUncompressedBytes",
        maximumUncompressedBytes,
      ),
      acceptCompressed,
    }),
  });
}

function deliveryEvidence(value: string): DeliveryEvidenceKind {
  if (value === "proofExplicit") {
    return "ExplicitProof";
  }
  if (value === "proofImplicit") {
    return "ImplicitProof";
  }
  if (value === "response") {
    return "Response";
  }
  throw new contract.PrnsValidationError(
    "InvalidEnum",
    `native delivery evidence is unknown: ${value}`,
  );
}

function commandBitrate(value: number | undefined): Bitrate {
  return value === undefined
    ? casework.Tag("Auto")
    : casework.Tag("BitsPerSecond", {
        value: positiveInteger("bitrateBps", value),
      });
}

function bitrateBitsPerSecond(bitrate: Bitrate): number | undefined {
  return casework.match(bitrate, {
    Auto: () => undefined,
    BitsPerSecond: ({ value }) => {
      const selected = positiveInteger("bitrate", value);
      if (selected < 5) {
        throw new contract.PrnsValidationError(
          "InvalidNumber",
          "bitrate must be at least 5 bits per second",
        );
      }
      return selected;
    },
  });
}

function rawInterfaceConfig(config: InterfaceConfig): RawInterfaceConfig {
  return casework.match_into<RawInterfaceConfig>().from(config, {
    AutoLan: ({
      groupId,
      discoveryScope,
      discoveryPort,
      dataPort,
      devices,
      ignoredDevices,
      multicastAddressType,
    }) => ({
      kind: "AutoLan",
      groupId,
      discoveryScope,
      discoveryPort,
      dataPort,
      devices: Array.from(devices),
      ignoredDevices: Array.from(ignoredDevices),
      multicastAddressType,
    }),
    TcpClient: ({ target, bitrate }) => ({
      kind: "TcpClient",
      target,
      bitrateBps: bitrateBitsPerSecond(bitrate),
    }),
    TcpServer: ({ bind, bitrate }) => ({
      kind: "TcpServer",
      bind,
      bitrateBps: bitrateBitsPerSecond(bitrate),
    }),
    Udp: ({ local, peer, bitrate }) => ({
      kind: "Udp",
      local,
      peer,
      bitrateBps: bitrateBitsPerSecond(bitrate),
    }),
    Serial: ({ port, line }) => ({
      kind: "Serial",
      port,
      line: rawSerialLine(line),
    }),
    Kiss: ({
      port,
      line,
      flowControl,
      preambleMillis,
      transmitTailMillis,
      persistence,
      slotTimeMillis,
      stationCallsign,
      stationIntervalSeconds,
    }) => ({
      kind: "Kiss",
      port,
      line: rawSerialLine(line),
      flowControl,
      preambleMillis,
      transmitTailMillis,
      persistence,
      slotTimeMillis,
      stationCallsign,
      stationIntervalSeconds,
    }),
    Ax25Kiss: ({
      port,
      line,
      flowControl,
      preambleMillis,
      transmitTailMillis,
      persistence,
      slotTimeMillis,
      callsign,
      ssid,
    }) => ({
      kind: "Ax25Kiss",
      port,
      line: rawSerialLine(line),
      flowControl,
      preambleMillis,
      transmitTailMillis,
      persistence,
      slotTimeMillis,
      callsign,
      ssid,
    }),
    RNode: ({
      port,
      radio,
      flowControl,
      stationCallsign,
      stationIntervalSeconds,
      airtimeLimitShortCentiPercent,
      airtimeLimitLongCentiPercent,
    }) => ({
      kind: "RNode",
      port,
      radio: rawRNodeRadio(radio),
      flowControl,
      stationCallsign,
      stationIntervalSeconds,
      airtimeLimitShortCentiPercent,
      airtimeLimitLongCentiPercent,
    }),
    MultiRNode: ({
      port,
      stationCallsign,
      stationIntervalSeconds,
      members,
    }) => ({
      kind: "MultiRNode",
      port,
      stationCallsign,
      stationIntervalSeconds,
      members: members.map((member) => ({
        name: member.name,
        virtualPort: member.virtualPort,
        radio: rawRNodeRadio(member.radio),
        flowControl: member.flowControl,
        outgoing: member.outgoing,
      })),
    }),
    Pipe: ({ command, respawnDelayMillis }) => ({
      kind: "Pipe",
      command: Array.from(command),
      respawnDelayMillis,
    }),
    BackboneClient: ({ target, bitrate }) => ({
      kind: "BackboneClient",
      target,
      bitrateBps: bitrateBitsPerSecond(bitrate),
    }),
    BackboneServer: ({ bind, bitrate }) => ({
      kind: "BackboneServer",
      bind,
      bitrateBps: bitrateBitsPerSecond(bitrate),
    }),
    I2p: ({ peers, connectable }) => ({
      kind: "I2p",
      peers: Array.from(peers),
      connectable,
    }),
    Weave: ({ port }) => ({ kind: "Weave", port }),
    AutomaticUsb: () => ({ kind: "AutomaticUsb" }),
    AutomaticBluetoothLe: () => ({ kind: "AutomaticBluetoothLe" }),
    WebSocketClient: ({ target, framing }) => ({
      kind: "WebSocketClient",
      target,
      framing,
    }),
    WebSocketServer: ({ bind, framing }) => ({
      kind: "WebSocketServer",
      bind,
      framing,
    }),
    BrowserRendezvous: ({ url }) => ({ kind: "BrowserRendezvous", url }),
  });
}

function rawInterfaceRoutingPolicy(
  routing: InterfaceRoutingPolicy | undefined,
): RawInterfaceRoutingPolicy | undefined {
  if (routing === undefined) return undefined;
  if (routing.gravity !== undefined && !Number.isSafeInteger(routing.gravity)) {
    invalidConfiguration("gravity must be a safe integer");
  }
  return {
    ...(routing.mode === undefined ? {} : { mode: routing.mode }),
    ...(routing.gravity === undefined ? {} : { gravity: routing.gravity }),
    ...(routing.recursivePathRequests === undefined
      ? {}
      : { recursivePathRequests: routing.recursivePathRequests }),
    ...(routing.announcesFromInternal === undefined
      ? {}
      : { announcesFromInternal: routing.announcesFromInternal }),
    ...(routing.announcesToInternal === undefined
      ? {}
      : { announcesToInternal: routing.announcesToInternal }),
  };
}

function rawSerialLine(
  line: import("../contract.js").SerialLineConfig,
): RawSerialLine {
  return {
    baud: line.baud,
    dataBits: line.dataBits,
    parity: line.parity,
    stopBits: line.stopBits,
  };
}

function rawRNodeRadio(
  radio: import("../contract.js").RNodeRadioConfig,
): RawRNodeRadio {
  return {
    frequencyHz: radio.frequencyHz,
    bandwidthHz: radio.bandwidthHz,
    txPowerDbm: radio.txPowerDbm,
    spreadingFactor: radio.spreadingFactor,
    codingRate: radio.codingRate,
  };
}

function validateInterfaceConfig(config: InterfaceConfig): void {
  casework.match(config, {
    AutoLan: ({
      groupId,
      discoveryPort,
      dataPort,
      devices,
      ignoredDevices,
    }) => {
      if (groupId !== undefined) nonEmpty("groupId", groupId);
      if (discoveryPort !== undefined) {
        boundedInteger("discoveryPort", discoveryPort, 1, 65_534);
      }
      if (dataPort !== undefined) {
        boundedInteger("dataPort", dataPort, 1, 65_535);
      }
      stringArray("devices", devices);
      stringArray("ignoredDevices", ignoredDevices);
    },
    TcpClient: ({ target, bitrate }) => {
      nonEmpty("target", target);
      bitrateBitsPerSecond(bitrate);
    },
    TcpServer: ({ bind, bitrate }) => {
      nonEmpty("bind", bind);
      bitrateBitsPerSecond(bitrate);
    },
    Udp: ({ local, peer, bitrate }) => {
      nonEmpty("local", local);
      nonEmpty("peer", peer);
      bitrateBitsPerSecond(bitrate);
    },
    Serial: ({ port, line }) => {
      nonEmpty("port", port);
      validateSerialLine(line);
    },
    Kiss: ({
      port,
      line,
      preambleMillis,
      transmitTailMillis,
      persistence,
      slotTimeMillis,
      stationCallsign,
      stationIntervalSeconds,
    }) => {
      nonEmpty("port", port);
      validateSerialLine(line);
      boundedInteger("preambleMillis", preambleMillis, 0, 0xffff_ffff);
      boundedInteger("transmitTailMillis", transmitTailMillis, 0, 0xffff_ffff);
      boundedInteger("persistence", persistence, 0, 255);
      boundedInteger("slotTimeMillis", slotTimeMillis, 0, 0xffff_ffff);
      if (stationCallsign !== undefined) validateCallsign(stationCallsign);
      if (stationIntervalSeconds !== undefined) {
        nonNegativeInteger("stationIntervalSeconds", stationIntervalSeconds);
      }
    },
    Ax25Kiss: ({
      port,
      line,
      preambleMillis,
      transmitTailMillis,
      persistence,
      slotTimeMillis,
      callsign,
      ssid,
    }) => {
      nonEmpty("port", port);
      validateSerialLine(line);
      boundedInteger("preambleMillis", preambleMillis, 0, 0xffff_ffff);
      boundedInteger("transmitTailMillis", transmitTailMillis, 0, 0xffff_ffff);
      boundedInteger("persistence", persistence, 0, 255);
      boundedInteger("slotTimeMillis", slotTimeMillis, 0, 0xffff_ffff);
      validateCallsign(callsign);
      boundedInteger("ssid", ssid, 0, 15);
    },
    RNode: ({
      port,
      radio,
      stationCallsign,
      stationIntervalSeconds,
      airtimeLimitShortCentiPercent,
      airtimeLimitLongCentiPercent,
    }) => {
      nonEmpty("port", port);
      validateRadio(radio);
      if (stationCallsign !== undefined) validateCallsign(stationCallsign);
      if (stationIntervalSeconds !== undefined) {
        nonNegativeInteger("stationIntervalSeconds", stationIntervalSeconds);
      }
      if (airtimeLimitShortCentiPercent !== undefined) {
        boundedInteger(
          "airtimeLimitShortCentiPercent",
          airtimeLimitShortCentiPercent,
          0,
          65_535,
        );
      }
      if (airtimeLimitLongCentiPercent !== undefined) {
        boundedInteger(
          "airtimeLimitLongCentiPercent",
          airtimeLimitLongCentiPercent,
          0,
          65_535,
        );
      }
    },
    MultiRNode: ({
      port,
      stationCallsign,
      stationIntervalSeconds,
      members,
    }) => {
      nonEmpty("port", port);
      if (stationCallsign !== undefined) validateCallsign(stationCallsign);
      if (stationIntervalSeconds !== undefined) {
        nonNegativeInteger("stationIntervalSeconds", stationIntervalSeconds);
      }
      if (members.length === 0) invalidConfiguration("members must not be empty");
      for (const member of members) {
        nonEmpty("member name", member.name);
        boundedInteger("virtualPort", member.virtualPort, 0, 255);
        validateRadio(member.radio);
      }
    },
    Pipe: ({ command, respawnDelayMillis }) => {
      if (command.length === 0) invalidConfiguration("command must not be empty");
      stringArray("command", command);
      nonNegativeInteger("respawnDelayMillis", respawnDelayMillis);
    },
    BackboneClient: ({ target, bitrate }) => {
      nonEmpty("target", target);
      bitrateBitsPerSecond(bitrate);
    },
    BackboneServer: ({ bind, bitrate }) => {
      nonEmpty("bind", bind);
      bitrateBitsPerSecond(bitrate);
    },
    I2p: ({ peers }) => stringArray("peers", peers),
    Weave: ({ port }) => {
      nonEmpty("port", port);
    },
    AutomaticUsb: () => undefined,
    AutomaticBluetoothLe: () => undefined,
    WebSocketClient: ({ target, framing }) => {
      validateWebSocket("target", target);
      validateWebSocketFramingSelection(framing);
    },
    WebSocketServer: ({ bind, framing }) => {
      nonEmpty("bind", bind);
      validateWebSocketFramingSelection(framing);
    },
    BrowserRendezvous: ({ url }) => validateWebSocket("url", url),
  });
}

function validateSerialLine(line: import("../contract.js").SerialLineConfig): void {
  boundedInteger("baud", line.baud, 1, 0xffff_ffff);
  if (!["Five", "Six", "Seven", "Eight"].includes(line.dataBits)) {
    invalidConfiguration("dataBits is invalid");
  }
  if (!["None", "Even", "Odd"].includes(line.parity)) {
    invalidConfiguration("parity is invalid");
  }
  if (!["One", "Two"].includes(line.stopBits)) {
    invalidConfiguration("stopBits is invalid");
  }
}

function validateRadio(radio: import("../contract.js").RNodeRadioConfig): void {
  positiveInteger("frequencyHz", radio.frequencyHz);
  boundedInteger("bandwidthHz", radio.bandwidthHz, 1, 0xffff_ffff);
  boundedInteger("txPowerDbm", radio.txPowerDbm, -32_768, 32_767);
  boundedInteger("spreadingFactor", radio.spreadingFactor, 5, 12);
  boundedInteger("codingRate", radio.codingRate, 5, 8);
}

function validateCallsign(value: string): void {
  if (!/^[A-Za-z0-9]{1,6}$/.test(value)) {
    invalidConfiguration("callsign must contain 1 to 6 ASCII alphanumeric characters");
  }
}

function validateWebSocket(name: string, value: string): void {
  const scheme = value.startsWith("ws://")
    ? "ws://"
    : value.startsWith("wss://")
      ? "wss://"
      : undefined;
  if (
    scheme === undefined ||
    value.length === scheme.length ||
    /\s/.test(value)
  ) {
    invalidConfiguration(`${name} must be a ws:// or wss:// URL with no whitespace`);
  }
}

function validateWebSocketFramingSelection(value: unknown): void {
  if (!contract.isWebSocketFramingSelection(value)) {
    invalidConfiguration("framing is invalid");
  }
}

function stringArray(name: string, values: readonly string[]): void {
  for (const value of values) nonEmpty(name, value);
}

function boundedInteger(name: string, value: number, minimum: number, maximum: number): number {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    invalidConfiguration(`${name} must be an integer from ${minimum} through ${maximum}`);
  }
  return value;
}

function invalidConfiguration(detail: string): never {
  throw new contract.PrnsValidationError("InvalidNumber", detail);
}

function interfaceKey(interfaceId: InterfaceId): string {
  return Array.from(interfaceId, (value) => value.toString(16).padStart(2, "0")).join("");
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

function nonNegativeInteger(name: string, value: number): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new contract.PrnsValidationError(
      "InvalidNumber",
      `${name} must be a non-negative safe integer`,
    );
  }
  return value;
}

function rawSafeUint(name: string, value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new contract.PrnsValidationError(
      "InvalidNumber",
      `${name} must be a non-negative safe integer`,
    );
  }
  return value;
}

function nonNegativeBigInt(name: string, value: unknown): bigint {
  if (typeof value !== "bigint" || value < 0n) {
    throw new contract.PrnsValidationError(
      "InvalidNumber",
      `${name} must be a non-negative bigint`,
    );
  }
  return value;
}

function exactBytesAsSafeNumber(name: string, value: bigint): number {
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new contract.PrnsValidationError(
      "InvalidNumber",
      `${name} exceeds the JavaScript safe-integer limit`,
    );
  }
  return Number(value);
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
