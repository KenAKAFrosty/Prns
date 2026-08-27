import { Tag } from "../casework.js";
import { interfaceId } from "../contract.js";
import type {
  InterfaceId,
  InterfaceKind,
  InterfaceRoutingPolicy,
  WebSocketFramingSelection,
} from "../contract.js";
import { byteKey } from "./bytes.js";
import { describeHostError } from "./host_errors.js";
import type { BrowserUsbDeviceFilter } from "./host_apis.js";
import {
  outboundTargets,
  parseOutboundFrame,
} from "./outbound.js";
import type { PrnsOutboundFrame } from "./outbound.js";
import {
  MIN_ENTROPY_BYTES,
  PrnsValidationError,
  bitrateBps,
  channelTag,
  hardwareMtu,
  packetFrame,
  positiveInteger,
} from "./values.js";
import type {
  BitrateBps,
  HardwareMtu,
  InstantMillis,
  PacketFrame,
} from "./values.js";
import type { WebSocketRuntimeRegistration } from "./websocket/index.js";
import type {
  BleIdentityAvailability,
  BluetoothReassemblerBinding,
  EntropyFailure,
  EntropyOutcome,
  EntropySource,
  InterfaceName,
  PrnsRuntimeBinding,
  PrnsWasmModule,
  RuntimeInterfaceKind,
  RuntimeOperation,
  RuntimeRegisterInterfaceOptions,
  RuntimeRejected,
  StableIdentityUnavailable,
  UsbAutoDecoderBinding,
  WebSocketFramingCodecBinding,
} from "./runtime_contract.js";

type InterfaceRegistrationOutcome<Name extends InterfaceName> =
  | Tag<"Registered", InterfaceId>
  | Tag<"AlreadyActive", { readonly interface: Name; readonly target: string }>
  | RuntimeRejected;

type HostedInterfaceRegistration<Name extends InterfaceName> =
  RuntimeRegisterInterfaceOptions & {
    readonly interfaceName: Name;
    readonly supervisorKind?: RuntimeInterfaceKind;
    readonly contractKind?: InterfaceKind;
  };

type RuntimeInterfaceInspection = {
  readonly id: InterfaceId;
  readonly name: InterfaceName;
  readonly kind?: InterfaceKind;
  readonly rxBytes: number;
  readonly txBytes: number;
};

type InterfaceDetachOutcome = Tag<"Detached"> | RuntimeRejected;
type RuntimeReadyOutcome = Tag<"Ready"> | RuntimeRejected;
type RuntimeIngestOutcome = Tag<"Accepted"> | EntropyFailure | RuntimeRejected;
type OutboundTakeOutcome =
  | Tag<"Outbound", readonly PrnsOutboundFrame[]>
  | Tag<"OutboundQueueFull", { readonly capacity: number }>
  | RuntimeRejected;
type RuntimeOutboundDrainOutcome =
  | Tag<"Drained", readonly PrnsOutboundFrame[]>
  | RuntimeRejected;
type OutboundActivityOutcome =
  | Tag<"RuntimeAdvanced">
  | Tag<"InterfaceDetached">;
type OutboundActivityWaiter = (outcome: OutboundActivityOutcome) => void;

const INTERFACE_OUTBOUND_QUEUE_DEPTH = 64;

export class RuntimeHost {
  readonly #wasm: PrnsWasmModule;
  readonly #runtime: PrnsRuntimeBinding;
  readonly #entropy: EntropySource;
  readonly #now: () => InstantMillis;
  readonly #bleIdentityAvailability: BleIdentityAvailability;
  readonly #onRuntimeActivity: () => void;
  #activeInterfaces = new Map<
    string,
    {
      id: InterfaceId;
      name: InterfaceName;
      contractKind?: InterfaceKind;
      registrationKey: string;
      supervisorKind: RuntimeInterfaceKind;
      rxBytes: number;
      txBytes: number;
    }
  >();
  #activeRegistrationKeys = new Set<string>();
  #outboundQueues = new Map<string, PrnsOutboundFrame[]>();
  #overflowedOutbound = new Set<string>();
  #outboundActivityWaiters = new Map<string, Set<OutboundActivityWaiter>>();

  constructor(
    wasm: PrnsWasmModule,
    runtime: PrnsRuntimeBinding,
    entropy: EntropySource,
    now: () => InstantMillis,
    bleIdentityAvailability: BleIdentityAvailability,
    onRuntimeActivity: () => void,
  ) {
    this.#wasm = wasm;
    this.#runtime = runtime;
    this.#entropy = entropy;
    this.#now = now;
    this.#bleIdentityAvailability = bleIdentityAvailability;
    this.#onRuntimeActivity = onRuntimeActivity;
  }

  runtimeReadiness(): RuntimeReadyOutcome {
    try {
      this.#runtime.snapshot();
      return Tag("Ready");
    } catch (error) {
      return runtimeRejected("inspect-readiness", error);
    }
  }

  registerInterface<Name extends InterfaceName>(
    registration: HostedInterfaceRegistration<Name>,
  ): InterfaceRegistrationOutcome<Name> {
    const {
      interfaceName,
      supervisorKind = registration.kind,
      contractKind = stableInterfaceKind(registration.kind),
      ...options
    } = registration;
    const registrationKey = `${options.kind}:${byteKey(options.channelTag)}`;
    if (this.#activeRegistrationKeys.has(registrationKey)) {
      return Tag("AlreadyActive", {
        interface: interfaceName,
        target: registrationKey,
      });
    }
    let id: InterfaceId;
    try {
      id = interfaceId(
        this.#runtime.registerInterface({ ...options, nowMs: this.#now() }),
      );
    } catch (error) {
      return runtimeRejected("register-interface", error);
    }
    const key = byteKey(id);
    if (this.#activeInterfaces.has(key)) {
      return Tag("AlreadyActive", {
        interface: interfaceName,
        target: key,
      });
    }
    this.#activeRegistrationKeys.add(registrationKey);
    this.#activeInterfaces.set(key, {
      id,
      name: interfaceName,
      ...(contractKind === undefined ? {} : { contractKind }),
      registrationKey,
      supervisorKind,
      rxBytes: 0,
      txBytes: 0,
    });
    this.#outboundQueues.set(key, []);
    return Tag("Registered", id);
  }

  deactivateInterface(id: InterfaceId): InterfaceDetachOutcome {
    const key = byteKey(id);
    const active = this.#activeInterfaces.get(key);
    if (!active) {
      this.#resolveOutboundActivity(key, Tag("InterfaceDetached"));
      return Tag("Detached");
    }
    try {
      const removed = this.#runtime.removeInterface({
        interfaceId: id,
        nowMs: this.#now(),
      });
      if (!removed) {
        return runtimeRejected(
          "remove-interface",
          `runtime did not contain interface ${key}`,
        );
      }
    } catch (error) {
      return runtimeRejected("remove-interface", error);
    }
    this.#activeInterfaces.delete(key);
    this.#activeRegistrationKeys.delete(active.registrationKey);
    this.#outboundQueues.delete(key);
    this.#overflowedOutbound.delete(key);
    this.#resolveOutboundActivity(key, Tag("InterfaceDetached"));
    return Tag("Detached");
  }

  setContractKind(id: InterfaceId, kind: InterfaceKind): void {
    const active = this.#activeInterfaces.get(byteKey(id));
    if (active !== undefined) {
      active.contractKind = kind;
    }
  }

  interfaceInspection(): ReadonlyMap<string, RuntimeInterfaceInspection> {
    return new Map(
      [...this.#activeInterfaces].map(([key, active]) => [
        key,
        {
          id: active.id,
          name: active.name,
          ...(active.contractKind === undefined
            ? {}
            : { kind: active.contractKind }),
          rxBytes: active.rxBytes,
          txBytes: active.txBytes,
        },
      ]),
    );
  }

  ingest(interfaceId: InterfaceId, bytes: PacketFrame): RuntimeIngestOutcome {
    const entropy = this.entropy();
    if (entropy.tag !== "Filled") {
      return entropy;
    }
    try {
      this.#runtime.ingest({
        interfaceId,
        bytes,
        nowMs: this.#now(),
        entropy: entropy.data,
      });
      const active = this.#activeInterfaces.get(byteKey(interfaceId));
      if (active !== undefined) {
        active.rxBytes = saturatingAdd(active.rxBytes, bytes.length);
      }
      this.notifyRuntimeActivity();
      return Tag("Accepted");
    } catch (error) {
      return runtimeRejected("ingest", error);
    }
  }

  drainOutbound(): RuntimeOutboundDrainOutcome {
    try {
      return Tag("Drained", this.#runtime.drainOutbound().map(parseOutboundFrame));
    } catch (error) {
      return runtimeRejected("drain-outbound", error);
    }
  }

  takeOutboundFor(
    interfaceId: InterfaceId,
    maximumFrames = Number.MAX_SAFE_INTEGER,
  ): OutboundTakeOutcome {
    const interfaceKey = byteKey(interfaceId);
    const direct: PrnsOutboundFrame[] = [];
    const drained = this.drainOutbound();
    if (drained.tag !== "Drained") {
      return drained;
    }
    for (const frame of drained.data) {
      for (const [key, active] of this.#activeInterfaces) {
        if (outboundTargets(frame.target, active.id, active.supervisorKind)) {
          if (key === interfaceKey) {
            direct.push(frame);
            continue;
          }
          const queue = this.#outboundQueues.get(key);
          if (queue && queue.length < INTERFACE_OUTBOUND_QUEUE_DEPTH) {
            queue.push(frame);
          } else if (queue) {
            this.#overflowedOutbound.add(key);
          }
        }
      }
    }
    if (this.#overflowedOutbound.delete(interfaceKey)) {
      this.#outboundQueues.set(interfaceKey, []);
      return Tag("OutboundQueueFull", {
        capacity: INTERFACE_OUTBOUND_QUEUE_DEPTH,
      });
    }
    const queued = this.#outboundQueues.get(interfaceKey) ?? [];
    const available = queued.concat(direct);
    const outbound = available.slice(0, maximumFrames);
    this.#outboundQueues.set(interfaceKey, available.slice(maximumFrames));
    const active = this.#activeInterfaces.get(interfaceKey);
    if (active !== undefined) {
      active.txBytes = outbound.reduce(
        (total, frame) => saturatingAdd(total, frame.bytes.length),
        active.txBytes,
      );
    }
    return Tag("Outbound", outbound);
  }

  waitForOutboundActivity(id: InterfaceId): Promise<OutboundActivityOutcome> {
    const key = byteKey(id);
    if (!this.#activeInterfaces.has(key)) {
      return Promise.resolve(Tag("InterfaceDetached"));
    }
    return new Promise((resolve) => {
      const waiters = this.#outboundActivityWaiters.get(key) ?? new Set();
      waiters.add(resolve);
      this.#outboundActivityWaiters.set(key, waiters);
    });
  }

  notifyRuntimeActivity(): void {
    const keys = [...this.#outboundActivityWaiters.keys()];
    for (const key of keys) {
      this.#resolveOutboundActivity(key, Tag("RuntimeAdvanced"));
    }
    this.#onRuntimeActivity();
  }

  #resolveOutboundActivity(
    key: string,
    outcome: OutboundActivityOutcome,
  ): void {
    const waiters = this.#outboundActivityWaiters.get(key);
    if (waiters === undefined) {
      return;
    }
    this.#outboundActivityWaiters.delete(key);
    for (const resolve of waiters) {
      resolve(outcome);
    }
  }

  createUsbAutoDecoder(): UsbAutoDecoderBinding {
    return new this.#wasm.UsbAutoDecoder();
  }

  createBluetoothReassembler(): BluetoothReassemblerBinding {
    return new this.#wasm.BluetoothReassembler();
  }

  createWebSocketFramingCodec(
    selection: WebSocketFramingSelection,
  ): WebSocketFramingCodecBinding {
    return new this.#wasm.WebSocketFramingCodec(
      wasmWebSocketFramingSelection(selection),
    );
  }

  bluetoothServiceUuid(): string {
    return this.#wasm.bluetoothServiceUuid();
  }

  bluetoothIdentityReadiness():
    | Tag<"Ready">
    | StableIdentityUnavailable<"bluetooth"> {
    return this.#bleIdentityAvailability.tag === "Available"
      ? Tag("Ready")
      : this.#bleIdentityAvailability;
  }

  bluetoothControlUuid(): string {
    return this.#wasm.bluetoothControlUuid();
  }

  bluetoothDataUuid(): string {
    return this.#wasm.bluetoothDataUuid();
  }

  bluetoothBitrateBps(): BitrateBps {
    return bitrateBps(this.#wasm.bluetoothBitrateBps());
  }

  bluetoothHardwareMtu(): HardwareMtu {
    return hardwareMtu(this.#wasm.bluetoothHardwareMtu());
  }

  bluetoothDialerHello(): Uint8Array {
    return this.#wasm.bluetoothDialerHello(this.#runtime.bluetoothIdentity());
  }

  bluetoothDecodeControl(bytes: Uint8Array): unknown {
    return this.#wasm.bluetoothDecodeControl(bytes);
  }

  bluetoothDataFragments(packet: PacketFrame): Uint8Array[] {
    return this.#wasm.bluetoothDataFragments(packet);
  }

  websocketBitrateBps(): BitrateBps {
    return bitrateBps(this.#wasm.websocketBitrateBps());
  }

  websocketFrameCap(): number {
    return positiveInteger(this.#wasm.websocketFrameCap(), "WebSocket frame cap");
  }

  websocketHardwareMtu(): HardwareMtu {
    return hardwareMtu(this.#wasm.websocketHardwareMtu());
  }

  webSocketRegister(
    options: WebSocketRuntimeRegistration,
  ): InterfaceRegistrationOutcome<"websocket"> {
    try {
      return this.registerInterface({
        interfaceName: "websocket",
        kind: "websocket-client",
        channelTag: channelTag(options.channelTag),
        bitrateBps: options.bitrateBps,
        hardwareMtu: options.hardwareMtu,
        ...runtimeInterfaceRouting(options.routing),
      });
    } catch (error) {
      return runtimeRejected("register-interface", error);
    }
  }

  webSocketIngest(
    id: InterfaceId,
    bytes: Uint8Array,
  ): RuntimeIngestOutcome {
    try {
      return this.ingest(id, packetFrame(bytes));
    } catch (error) {
      return runtimeRejected("ingest", error);
    }
  }

  autoWifiReady(): RuntimeReadyOutcome {
    return this.runtimeReadiness();
  }

  autoWifiRegister(id: Uint8Array): InterfaceRegistrationOutcome<"auto-wifi"> {
    try {
      return this.registerInterface({
        interfaceName: "auto-wifi",
        kind: "auto-wifi",
        channelTag: channelTag(id),
        bitrateBps: this.websocketBitrateBps(),
        hardwareMtu: this.websocketHardwareMtu(),
      });
    } catch (error) {
      return runtimeRejected("register-interface", error);
    }
  }

  autoWifiDeactivate(id: InterfaceId): InterfaceDetachOutcome {
    return this.deactivateInterface(id);
  }

  autoWifiIngest(id: InterfaceId, bytes: Uint8Array): RuntimeIngestOutcome {
    try {
      return this.ingest(id, packetFrame(bytes));
    } catch (error) {
      return runtimeRejected("ingest", error);
    }
  }

  autoWifiTakeOutbound(id: InterfaceId): OutboundTakeOutcome {
    return this.takeOutboundFor(id);
  }

  autoWifiBitrateBps(): BitrateBps {
    return this.websocketBitrateBps();
  }

  autoWifiHardwareMtu(): HardwareMtu {
    return this.websocketHardwareMtu();
  }

  autoWifiFrameCap(): number {
    return this.websocketFrameCap();
  }

  usbAutoHostBitrateBps(): BitrateBps {
    return bitrateBps(this.#wasm.usbAutoHostBitrateBps());
  }

  usbAutoHostHardwareMtu(): HardwareMtu {
    return hardwareMtu(this.#wasm.usbAutoHostHardwareMtu());
  }

  defaultUsbAutoFilters(): readonly BrowserUsbDeviceFilter[] {
    return [
      {
        vendorId: this.#wasm.usbAutoWebUsbVendorId(),
        productId: this.#wasm.usbAutoWebUsbProductId(),
      },
    ];
  }

  usbAutoNodeTagFor(interfaceId: InterfaceId): Uint8Array {
    return this.#wasm.usbAutoNodeTagFor(interfaceId);
  }

  usbAutoHostHelloFrame(): Uint8Array {
    return this.#wasm.usbAutoHostHelloFrame();
  }

  usbAutoHostHelloAckFrame(nodeTag: Uint8Array): Uint8Array {
    return this.#wasm.usbAutoHostHelloAckFrame(nodeTag);
  }

  usbAutoDataFrame(packet: PacketFrame): Uint8Array {
    return this.#wasm.usbAutoDataFrame(packet);
  }

  entropy(): EntropyOutcome {
    return fillEntropy(this.#entropy, MIN_ENTROPY_BYTES);
  }
}

export function runtimeRejected(
  operation: RuntimeOperation,
  error: unknown,
): RuntimeRejected {
  return Tag("RuntimeRejected", {
    operation,
    detail: describeHostError(error),
  });
}

export function fillEntropy(
  source: EntropySource,
  length: number,
): EntropyOutcome {
  let outcome: EntropyOutcome;
  try {
    outcome = source(length);
  } catch (error) {
    return Tag("EntropySourceFailed", { detail: describeHostError(error) });
  }
  if (outcome.tag !== "Filled") {
    return outcome;
  }
  if (outcome.data.length < length) {
    return Tag("InsufficientEntropy", {
      minimum: length,
      actual: outcome.data.length,
    });
  }
  return outcome;
}

export function saturatingAdd(left: number, right: number): number {
  return Math.min(Number.MAX_SAFE_INTEGER, left + right);
}

function runtimeInterfaceRouting(
  routing: InterfaceRoutingPolicy | undefined,
): Pick<
  RuntimeRegisterInterfaceOptions,
  | "mode"
  | "gravity"
  | "recursivePathRequests"
  | "announcesFromInternal"
  | "announcesToInternal"
> {
  if (routing === undefined) return {};
  if (routing.gravity !== undefined && !Number.isSafeInteger(routing.gravity)) {
    throw new PrnsValidationError(
      "invalid-number",
      "gravity must be a safe integer",
    );
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

function stableInterfaceKind(
  kind: RuntimeInterfaceKind,
): InterfaceKind | undefined {
  return ({
    "auto-usb-host": "AutomaticUsb",
    "auto-usb-device": "AutomaticUsb",
    rnode: "RNode",
    "bluetooth-auto": "AutomaticBluetoothLe",
    "bluetooth-peer": "AutomaticBluetoothLe",
    "auto-wifi": "BrowserRendezvous",
    "websocket-client": "WebSocketClient",
    "websocket-server": "WebSocketServer",
    "websocket-server-peer": "WebSocketServer",
    serial: "Serial",
    kiss: "Kiss",
    pipe: "Pipe",
  } satisfies Record<RuntimeInterfaceKind, InterfaceKind | undefined>)[kind];
}

function wasmWebSocketFramingSelection(
  selection: WebSocketFramingSelection,
): string {
  switch (selection) {
    case "Auto":
      return "auto";
    case "RawPacket":
      return "raw";
    case "Hdlc":
      return "hdlc";
    case "Kiss":
      return "kiss";
  }
  const unreachable: never = selection;
  return unreachable;
}
