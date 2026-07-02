import {
  Prns,
  bitrateBps,
  channelTag,
  destinationHash,
  hardwareMtu,
  identitySecretKey,
  interfaceId,
  nowMillis,
  packetFrame,
} from "../ts/index.js";
import type {
  BluetoothReassemblerBinding,
  DestinationHash,
  IdentitySecretKey,
  IdentityStore,
  InterfaceId,
  InterfaceSession,
  PacketFrame,
  PrnsRuntimeBinding,
  PrnsWasmModule,
  RuntimeAnnounceOptions,
  RuntimeIngestOptions,
  RuntimeRegisterInterfaceOptions,
  RuntimeRegisterSingleDestinationOptions,
  UsbAutoDecoderBinding,
} from "../ts/index.js";

const IDENTITY_LENGTH = 32;
const DEFAULT_WEBSOCKET_BITRATE = 1_000_000_000;
const DEFAULT_WEBSOCKET_MTU = 508;

class MockRuntime implements PrnsRuntimeBinding {
  readonly identity: IdentitySecretKey;
  readonly registered: RuntimeRegisterInterfaceOptions[] = [];
  readonly ingests: RuntimeIngestOptions[] = [];
  readonly destinations: RuntimeRegisterSingleDestinationOptions[] = [];
  outbound: unknown[] = [];

  constructor(identity: IdentitySecretKey) {
    this.identity = identity;
    lastRuntime = this;
  }

  registerInterface(options: RuntimeRegisterInterfaceOptions): InterfaceId {
    this.registered.push(options);
    return interfaceId(
      new Uint8Array([0, 0, 0, 0, 0, 0, 0, this.registered.length]),
    );
  }

  bluetoothIdentity(): Uint8Array {
    return this.identity;
  }

  registerSingleDestination(
    options: RuntimeRegisterSingleDestinationOptions,
  ): DestinationHash {
    this.destinations.push(options);
    return destinationHash(new Uint8Array(16).fill(this.destinations.length));
  }

  announce(_options: RuntimeAnnounceOptions): bigint {
    return 1n;
  }

  ingest(options: RuntimeIngestOptions): void {
    this.ingests.push(options);
  }

  drainEvents(): unknown[] {
    return [];
  }

  drainOutbound(): unknown[] {
    const outbound = this.outbound;
    this.outbound = [];
    return outbound;
  }

  snapshot(): unknown {
    return {
      type: "snapshot",
      ingestedPackets: this.ingests.length,
      ingestedCommands: 0,
      routes: 0,
      scheduledAnnounces: 0,
      interfaces: this.registered.map((options, index) => ({
        id: interfaceId(new Uint8Array([0, 0, 0, 0, 0, 0, 0, index + 1])),
        kind: options.kind,
        bitrateBps: options.bitrateBps,
        hardwareMtu: options.hardwareMtu,
        routes: 0,
        links: 0,
      })),
    };
  }
}

class MockUsbAutoDecoder implements UsbAutoDecoderBinding {
  feed(_chunk: Uint8Array): unknown[] {
    return [];
  }
}

class MockBluetoothReassembler implements BluetoothReassemblerBinding {
  absorb(_bytes: Uint8Array): Uint8Array | undefined {
    return undefined;
  }
}

class FakeWebSocket extends EventTarget {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static instances: FakeWebSocket[] = [];

  readonly url: string;
  readonly protocols: string | string[] | undefined;
  readonly sent: Uint8Array[] = [];
  readyState = FakeWebSocket.CONNECTING;
  binaryType: BinaryType = "blob";
  closeCalls = 0;

  constructor(url: string | URL, protocols?: string | string[]) {
    super();
    this.url = url.toString();
    this.protocols = protocols;
    FakeWebSocket.instances.push(this);
    queueMicrotask(() => {
      this.open();
    });
  }

  send(data: string | ArrayBufferLike | Blob | ArrayBufferView): void {
    assert(this.readyState === FakeWebSocket.OPEN, "socket is open when sending");
    this.sent.push(sendBytes(data));
  }

  close(): void {
    if (this.readyState === FakeWebSocket.CLOSED) {
      return;
    }
    this.readyState = FakeWebSocket.CLOSED;
    this.closeCalls += 1;
    this.dispatchEvent(new Event("close"));
  }

  emitMessage(data: MessageEvent["data"]): void {
    const event = new Event("message") as MessageEvent;
    Object.defineProperty(event, "data", { value: data });
    this.dispatchEvent(event);
  }

  private open(): void {
    if (this.readyState !== FakeWebSocket.CONNECTING) {
      return;
    }
    this.readyState = FakeWebSocket.OPEN;
    this.dispatchEvent(new Event("open"));
  }
}

let lastRuntime: MockRuntime | undefined;

async function main(): Promise<void> {
  const host = globalThis as typeof globalThis & { WebSocket?: typeof WebSocket };
  const previousWebSocket = host.WebSocket;
  host.WebSocket = FakeWebSocket as unknown as typeof WebSocket;

  try {
    const prns = await Prns.create({
      wasm: wasmModule(),
      identityStore: fixedIdentityStore(),
      entropy: fixedEntropy,
      now: () => nowMillis(123_456),
    });

    const customTag = channelTag(new TextEncoder().encode("websocket-smoke"));
    const customBitrate = bitrateBps(250_000);
    const customMtu = hardwareMtu(1200);
    const session = await prns.interfaces.webSocket.connect(
      "ws://127.0.0.1:9876/prns",
      {
        protocols: ["prns.v1"],
        channelTag: customTag,
        bitrateBps: customBitrate,
        hardwareMtu: customMtu,
      },
    );

    const socket = only(FakeWebSocket.instances, "one fake WebSocket was created");
    const runtime = assertDefined(lastRuntime, "mock runtime was constructed");
    const registered = only(runtime.registered, "one interface was registered");

    assert(socket.url === "ws://127.0.0.1:9876/prns", "target URL is preserved");
    assert(Array.isArray(socket.protocols), "subprotocol list is forwarded");
    assert(socket.protocols[0] === "prns.v1", "subprotocol value is preserved");
    assert(socket.binaryType === "arraybuffer", "binaryType is arraybuffer");
    assert(session.connected, "session reports connected");
    assert(session.state === "peer-confirmed", "open WebSocket is peer-confirmed");

    assert(registered.kind === "websocket-client", "websocket-client kind is used");
    assert(equalBytes(registered.channelTag, customTag), "channel tag override is used");
    assert(registered.bitrateBps === customBitrate, "bitrate override is used");
    assert(registered.hardwareMtu === customMtu, "MTU override is used");

    socket.emitMessage(arrayBuffer([1, 2, 3]));
    await settle();
    assertBytes(runtime.ingests[0]?.bytes, [1, 2, 3], "ArrayBuffer inbound ingests");

    socket.emitMessage(new Blob([new Uint8Array([4, 5, 6])]));
    await settle();
    assertBytes(runtime.ingests[1]?.bytes, [4, 5, 6], "Blob inbound ingests");

    runtime.outbound.push({
      type: "frame",
      target: { type: "interface", interfaceId: session.interfaceId },
      bytes: packetFrame(new Uint8Array([9, 8, 7])),
    });
    await waitFor(() => socket.sent.length === 1, "outbound frame was sent");
    assertBytes(socket.sent[0], [9, 8, 7], "outbound bytes are exact");

    socket.emitMessage("text is not a Prns frame");
    await settle();
    assert(sessionState(session) === "failed", "text frame fails the session");
    assert(session.failure?.code === "unsupported-frame", "text frame is rejected");
    assert(socket.closeCalls === 1, "failed session closes the socket");

    console.log("websocket smoke passed");
  } finally {
    if (previousWebSocket) {
      host.WebSocket = previousWebSocket;
    } else {
      delete host.WebSocket;
    }
  }
}

function wasmModule(): PrnsWasmModule {
  return {
    PrnsRuntime: MockRuntime,
    UsbAutoDecoder: MockUsbAutoDecoder,
    BluetoothReassembler: MockBluetoothReassembler,
    identitySecretKeyLength: () => IDENTITY_LENGTH,
    bluetoothServiceUuid: () => "00000000-0000-4000-8000-000000000001",
    bluetoothControlUuid: () => "00000000-0000-4000-8000-000000000002",
    bluetoothDataUuid: () => "00000000-0000-4000-8000-000000000003",
    bluetoothBitrateBps: () => 125_000,
    bluetoothHardwareMtu: () => 185,
    bluetoothDialerHello: () => new Uint8Array([1]),
    bluetoothDecodeControl: () => ({ type: "close", reason: "unused" }),
    bluetoothDataFragments: (packet: PacketFrame) => [packet],
    websocketBitrateBps: () => DEFAULT_WEBSOCKET_BITRATE,
    websocketHardwareMtu: () => DEFAULT_WEBSOCKET_MTU,
    usbAutoHostBitrateBps: () => 115_200,
    usbAutoHostHardwareMtu: () => 512,
    usbAutoWebUsbVendorId: () => 0x303a,
    usbAutoWebUsbProductId: () => 0x4001,
    usbAutoNodeTagFor: () => new Uint8Array([1, 2, 3, 4]),
    usbAutoHostHelloFrame: () => new Uint8Array([1]),
    usbAutoHostHelloAckFrame: () => new Uint8Array([2]),
    usbAutoDataFrame: (packet: PacketFrame) => packet,
  };
}

function fixedIdentityStore(): IdentityStore {
  return {
    load: async (expectedLength) =>
      identitySecretKey(new Uint8Array(expectedLength).fill(7), expectedLength),
    save: async () => {},
  };
}

function fixedEntropy(length: number): Uint8Array {
  return new Uint8Array(length).fill(42);
}

function sendBytes(data: string | ArrayBufferLike | Blob | ArrayBufferView): Uint8Array {
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data.slice(0));
  }
  if (ArrayBuffer.isView(data)) {
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  }
  throw new Error(`unexpected WebSocket send payload: ${typeof data}`);
}

function arrayBuffer(bytes: number[]): ArrayBuffer {
  const out = new ArrayBuffer(bytes.length);
  new Uint8Array(out).set(bytes);
  return out;
}

function assertBytes(
  actual: Uint8Array | undefined,
  expected: number[],
  message: string,
): void {
  assert(actual !== undefined, `${message}: actual bytes exist`);
  assert(equalBytes(actual, new Uint8Array(expected)), message);
}

function equalBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let i = 0; i < left.length; i += 1) {
    if (left[i] !== right[i]) {
      return false;
    }
  }
  return true;
}

function only<T>(items: readonly T[], message: string): T {
  assert(items.length === 1, message);
  return assertDefined(items[0], message);
}

function assertDefined<T>(value: T | undefined, message: string): T {
  assert(value !== undefined, message);
  return value;
}

function sessionState(session: InterfaceSession): InterfaceSession["state"] {
  return session.state;
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

function waitFor(predicate: () => boolean, message: string): Promise<void> {
  return new Promise((resolve, reject) => {
    let attempts = 0;
    const tick = (): void => {
      if (predicate()) {
        resolve();
        return;
      }
      attempts += 1;
      if (attempts > 30) {
        reject(new Error(message));
        return;
      }
      setTimeout(tick, 10);
    };
    tick();
  });
}

async function settle(): Promise<void> {
  await new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
}

await main();
