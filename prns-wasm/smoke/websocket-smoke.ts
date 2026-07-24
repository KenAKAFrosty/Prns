import {
  Prns,
  Tag,
  bitrateBps,
  channelTag,
  destinationHash,
  entropyBytes,
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
  InterfaceSessionStatus,
  PacketFrame,
  PrnsRuntimeBinding,
  PrnsCreateOutcome,
  PrnsWasmModule,
  RuntimeAnnounceOptions,
  RuntimeIngestOptions,
  RuntimeRegisterInterfaceInput,
  RuntimeRegisterNodePageOptions,
  RuntimeRemoveInterfaceInput,
  RuntimeRegisterSingleDestinationOptions,
  UsbAutoDecoderBinding,
  WebSocketConnectOutcome,
  WebSocketSession,
} from "../ts/index.js";

const IDENTITY_LENGTH = 32;
const DEFAULT_WEBSOCKET_BITRATE = 1_000_000_000;
const DEFAULT_WEBSOCKET_MTU = 508;
const WEBSOCKET_FRAME_CAP = 572;

class MockRuntime implements PrnsRuntimeBinding {
  readonly identity: IdentitySecretKey;
  readonly registered: RuntimeRegisterInterfaceInput[] = [];
  readonly removed: RuntimeRemoveInterfaceInput[] = [];
  readonly ingests: RuntimeIngestOptions[] = [];
  readonly destinations: RuntimeRegisterSingleDestinationOptions[] = [];
  outbound: unknown[] = [];
  registerFailure: Error | undefined;

  constructor(identity: IdentitySecretKey) {
    this.identity = identity;
    lastRuntime = this;
  }

  registerInterface(options: RuntimeRegisterInterfaceInput): InterfaceId {
    if (this.registerFailure) {
      throw this.registerFailure;
    }
    this.registered.push(options);
    return interfaceId(
      new Uint8Array([0, 0, 0, 0, 0, 0, 0, this.registered.length]),
    );
  }

  removeInterface(options: RuntimeRemoveInterfaceInput): boolean {
    this.removed.push(options);
    return true;
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

  registerNodePage(_options: RuntimeRegisterNodePageOptions): DestinationHash {
    return destinationHash(new Uint8Array(16).fill(0xff));
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
  readonly sentPayloads: (string | ArrayBufferLike | Blob | ArrayBufferView)[] = [];
  readyState = FakeWebSocket.CONNECTING;
  binaryType: BinaryType = "blob";
  bufferedAmount = 0;
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
    this.sentPayloads.push(data);
    this.sent.push(new Uint8Array(sendBytes(data)));
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

class DelayedBlob extends Blob {
  readonly #delayMs: number;

  constructor(bytes: Uint8Array, delayMs: number) {
    super([ownedArrayBuffer(bytes)]);
    this.#delayMs = delayMs;
  }

  override async arrayBuffer(): Promise<ArrayBuffer> {
    await wait(this.#delayMs);
    return super.arrayBuffer();
  }
}

let lastRuntime: MockRuntime | undefined;

async function main(): Promise<void> {
  const host = globalThis as typeof globalThis & { WebSocket?: typeof WebSocket };
  const previousWebSocket = host.WebSocket;
  host.WebSocket = FakeWebSocket as unknown as typeof WebSocket;

  try {
    const rejectedStore = await Prns.create({
      wasm: wasmModule(),
      identityStore: {
        load: async () => {
          throw new Error("load rejected");
        },
        save: async () => Tag("Saved"),
      },
    });
    assert(
      rejectedStore.tag === "IdentityStoreFailed",
      "identity-store rejection is contained",
    );

    const prns = expectReady(
      await Prns.create({
        wasm: wasmModule(),
        identityStore: fixedIdentityStore(),
        entropy: fixedEntropy,
        now: () => nowMillis(123_456),
      }),
    );

    const customTag = channelTag(new TextEncoder().encode("websocket-smoke"));
    const customBitrate = bitrateBps(250_000);
    const customMtu = hardwareMtu(1200);
    const invalidTarget = await prns.interfaces.webSocket.connect(
      "https://127.0.0.1:9876/not-websocket",
    );
    assert(invalidTarget.tag === "InvalidTarget", "invalid URL is semantically tagged");
    assert(socketCount() === 0, "invalid URL does not open a socket");
    const session = expectConnected(
      await prns.interfaces.webSocket.connect(
        "ws://127.0.0.1:9876/prns",
        {
          protocols: ["prns.v1"],
          channelTag: customTag,
          bitrateBps: customBitrate,
          hardwareMtu: customMtu,
        },
      ),
    );

    const socket = only(FakeWebSocket.instances, "one fake WebSocket was created");
    const runtime = assertDefined(lastRuntime, "mock runtime was constructed");
    const registered = only(runtime.registered, "one interface was registered");

    assert(socket.url === "ws://127.0.0.1:9876/prns", "target URL is preserved");
    assert(Array.isArray(socket.protocols), "subprotocol list is forwarded");
    assert(socket.protocols[0] === "prns.v1", "subprotocol value is preserved");
    assert(socket.binaryType === "arraybuffer", "binaryType is arraybuffer");
    assert(session.status.tag === "Active", "open WebSocket is active");

    assert(registered.kind === "websocket-client", "websocket-client kind is used");
    assert(equalBytes(registered.channelTag, customTag), "channel tag override is used");
    assert(registered.bitrateBps === customBitrate, "bitrate override is used");
    assert(registered.hardwareMtu === customMtu, "MTU override is used");

    const duplicate = await prns.interfaces.webSocket.connect(
      "ws://127.0.0.1:9876/prns",
      {
        protocols: ["prns.v1"],
        channelTag: customTag,
      },
    );
    assert(duplicate.tag === "AlreadyActive", "duplicate is semantically tagged");
    assert(socketCount() === 1, "duplicate resolves before opening");

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
    assert(socket.sentPayloads[0] instanceof Uint8Array, "outbound avoids a buffer copy");

    socket.bufferedAmount = 2 * 1024 * 1024;
    runtime.outbound.push({
      type: "frame",
      target: { type: "interface", interfaceId: session.interfaceId },
      bytes: packetFrame(new Uint8Array([7, 8, 9])),
    });
    await wait(40);
    assert(socket.sent.length === 1, "buffer pressure pauses outbound frames");
    socket.bufferedAmount = 0;
    await waitFor(() => socket.sent.length === 2, "buffer pressure recovers");

    socket.emitMessage(new DelayedBlob(new Uint8Array([10]), 20));
    socket.emitMessage(new DelayedBlob(new Uint8Array([11]), 0));
    await waitFor(() => runtime.ingests.length === 4, "Blob messages were ingested");
    assertBytes(runtime.ingests[2]?.bytes, [10], "slow Blob stays first");
    assertBytes(runtime.ingests[3]?.bytes, [11], "fast Blob stays second");

    socket.emitMessage("text is not a Prns frame");
    await settle();
    const failedStatus = sessionStatus(session);
    assert(failedStatus.tag === "Failed", "text frame fails the session");
    assert(
      failedStatus.data.tag === "UnsupportedFrame",
      "text frame is rejected",
    );
    assert(socket.closeCalls === 1, "failed session closes the socket");
    assert(runtime.removed.length === 1, "failed session removes its interface");

    const firstDefault = expectConnected(
      await prns.interfaces.webSocket.connect(
        "ws://127.0.0.1:9876/stable",
        { protocols: ["prns.v1"] },
      ),
    );
    const firstDefaultRegistration = assertDefined(
      runtime.registered[1],
      "first default registration exists",
    );
    await firstDefault.close();
    const secondDefault = expectConnected(
      await prns.interfaces.webSocket.connect(
        "ws://127.0.0.1:9876/stable",
        { protocols: ["prns.v1"] },
      ),
    );
    const secondDefaultRegistration = assertDefined(
      runtime.registered[2],
      "second default registration exists",
    );
    assert(
      equalBytes(
        firstDefaultRegistration.channelTag,
        secondDefaultRegistration.channelTag,
      ),
      "default channel tag is stable",
    );
    await secondDefault.close();

    const failedSocketIndex = FakeWebSocket.instances.length;
    runtime.registerFailure = new Error("registration failed");
    const registrationFailure = await prns.interfaces.webSocket.connect(
      "ws://127.0.0.1:9876/register-failure",
    );
    assert(
      registrationFailure.tag === "RuntimeRejected",
      "runtime registration failure is semantically tagged",
    );
    runtime.registerFailure = undefined;
    const failedSocket = assertDefined(
      FakeWebSocket.instances[failedSocketIndex],
      "registration failure socket exists",
    );
    assert(failedSocket.closeCalls === 1, "registration failure closes the socket");

    const fanoutA = expectConnected(
      await prns.interfaces.webSocket.connect(
        "ws://127.0.0.1:9876/fanout-a",
      ),
    );
    const fanoutSocketA = assertDefined(
      FakeWebSocket.instances.at(-1),
      "first fanout socket exists",
    );
    const fanoutB = expectConnected(
      await prns.interfaces.webSocket.connect(
        "ws://127.0.0.1:9876/fanout-b",
      ),
    );
    const fanoutSocketB = assertDefined(
      FakeWebSocket.instances.at(-1),
      "second fanout socket exists",
    );
    runtime.outbound.push({
      type: "frame",
      target: {
        type: "broadcast",
        supervisorKind: "websocket-client",
        fan: { type: "all" },
      },
      bytes: packetFrame(new Uint8Array([12, 13])),
    });
    await waitFor(
      () => fanoutSocketA.sent.length === 1 && fanoutSocketB.sent.length === 1,
      "broadcast reaches every WebSocket session",
    );
    assertBytes(fanoutSocketA.sent[0], [12, 13], "first fanout bytes are exact");
    assertBytes(fanoutSocketB.sent[0], [12, 13], "second fanout bytes are exact");
    fanoutSocketB.bufferedAmount = 2 * 1024 * 1024;
    runtime.outbound.push({
      type: "frame",
      target: { type: "interface", interfaceId: fanoutB.interfaceId },
      bytes: packetFrame(new Uint8Array([14])),
    });
    await wait(40);
    for (let index = 0; index < 65; index += 1) {
      runtime.outbound.push({
        type: "frame",
        target: {
          type: "broadcast",
          supervisorKind: "websocket-client",
          fan: { type: "all" },
        },
        bytes: packetFrame(new Uint8Array([index])),
      });
    }
    await waitFor(() => fanoutSocketA.sent.length === 66, "fast fanout keeps draining");
    fanoutSocketB.bufferedAmount = 0;
    await waitFor(
      () => fanoutB.status.tag === "Failed",
      "slow fanout queue is bounded",
    );
    assert(
      fanoutB.status.tag === "Failed" &&
        fanoutB.status.data.tag === "OutboundQueueFull",
      "slow fanout reports queue pressure",
    );
    await fanoutA.close();
    await fanoutB.close();

    const oversized = expectConnected(
      await prns.interfaces.webSocket.connect(
        "ws://127.0.0.1:9876/oversized",
      ),
    );
    const oversizedSocket = assertDefined(
      FakeWebSocket.instances.at(-1),
      "oversized test socket exists",
    );
    oversizedSocket.emitMessage(new ArrayBuffer(WEBSOCKET_FRAME_CAP + 1));
    await settle();
    assert(oversized.status.tag === "Failed", "oversized frame fails the session");
    assert(
      oversized.status.data.tag === "FrameTooLarge",
      "oversized frame is bounded",
    );

    delete host.WebSocket;
    const unavailable = await prns.interfaces.webSocket.connect(
      "ws://127.0.0.1:9876/unavailable",
    );
    assert(
      unavailable.tag === "HostApiUnavailable",
      "missing WebSocket API is semantically tagged",
    );
    host.WebSocket = FakeWebSocket as unknown as typeof WebSocket;

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
    websocketFrameCap: () => WEBSOCKET_FRAME_CAP,
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
      Tag(
        "Loaded",
        identitySecretKey(
          new Uint8Array(expectedLength).fill(7),
          expectedLength,
        ),
      ),
    save: async () => Tag("Saved"),
  };
}

function fixedEntropy(length: number) {
  return Tag("Filled", entropyBytes(new Uint8Array(length).fill(42)));
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

function ownedArrayBuffer(bytes: Uint8Array): ArrayBuffer {
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

function socketCount(): number {
  return FakeWebSocket.instances.length;
}

function assertDefined<T>(value: T | undefined, message: string): T {
  assert(value !== undefined, message);
  return value;
}

function expectReady(outcome: PrnsCreateOutcome): Prns {
  assert(outcome.tag === "Ready", `expected Ready, received ${outcome.tag}`);
  return outcome.data;
}

function expectConnected(outcome: WebSocketConnectOutcome): WebSocketSession {
  assert(
    outcome.tag === "Connected",
    `expected Connected, received ${outcome.tag}`,
  );
  return outcome.data;
}

function sessionStatus(session: InterfaceSession): InterfaceSessionStatus {
  return session.status;
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

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

async function settle(): Promise<void> {
  await wait(0);
}

await main();
