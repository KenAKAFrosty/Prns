import {
  BLE_IDENTITY_LENGTH,
  Prns,
  Tag,
  destinationHash,
  entropyBytes,
  identitySecretKey,
  interfaceId,
  nowMillis,
  parseBrowserGatewayCatalog,
  validateBrowserGatewayUrl,
} from "../ts/index.js";
import type {
  BleIdentity,
  BluetoothReassemblerBinding,
  BrowserRendezvousId,
  DestinationHash,
  IdentitySecretKey,
  InterfaceId,
  PrnsRuntimeBinding,
  PrnsWasmModule,
  RuntimeAnnounceOptions,
  RuntimeIngestOptions,
  RuntimeRegisterInterfaceInput,
  RuntimeRegisterNodePageOptions,
  RuntimeRegisterSingleDestinationOptions,
  RuntimeRemoveInterfaceInput,
  StableIdentityStore,
  UsbAutoDecoderBinding,
} from "../ts/index.js";

const IDENTITY_LENGTH = 32;
const LOCAL_ID = "01010101010101010101010101010101";
const ALIAS_ID = "02020202020202020202020202020202";
const REMOTE_IDS = [
  ALIAS_ID,
  "03030303030303030303030303030303",
  "04040404040404040404040404040404",
  "05050505050505050505050505050505",
] as const;

class MockRuntime implements PrnsRuntimeBinding {
  static latest: MockRuntime | undefined;
  readonly registered: RuntimeRegisterInterfaceInput[] = [];
  readonly removed: RuntimeRemoveInterfaceInput[] = [];
  readonly ingested: RuntimeIngestOptions[] = [];
  readonly #bleIdentity: Uint8Array;

  constructor(_identity: IdentitySecretKey, bleIdentity?: BleIdentity) {
    MockRuntime.latest = this;
    this.#bleIdentity = bleIdentity ?? new Uint8Array(BLE_IDENTITY_LENGTH);
  }

  registerInterface(options: RuntimeRegisterInterfaceInput): InterfaceId {
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
    return this.#bleIdentity;
  }

  registerSingleDestination(
    _options: RuntimeRegisterSingleDestinationOptions,
  ): DestinationHash {
    return destinationHash(new Uint8Array(16).fill(1));
  }

  registerNodePage(_options: RuntimeRegisterNodePageOptions): DestinationHash {
    return destinationHash(new Uint8Array(16).fill(2));
  }

  announce(_options: RuntimeAnnounceOptions): bigint {
    return 1n;
  }

  ingest(options: RuntimeIngestOptions): void {
    this.ingested.push(options);
  }

  drainEvents(): unknown[] {
    return [];
  }

  drainOutbound(): unknown[] {
    return [];
  }

  snapshot(): unknown {
    return { type: "snapshot" };
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

class MemoryStorage implements Storage {
  readonly #values = new Map<string, string>();

  get length(): number {
    return this.#values.size;
  }

  clear(): void {
    this.#values.clear();
  }

  getItem(key: string): string | null {
    return this.#values.get(key) ?? null;
  }

  key(index: number): string | null {
    return [...this.#values.keys()][index] ?? null;
  }

  removeItem(key: string): void {
    this.#values.delete(key);
  }

  setItem(key: string, value: string): void {
    this.#values.set(key, value);
  }
}

class GatewayWebSocket extends EventTarget {
  static readonly instances: GatewayWebSocket[] = [];
  readonly url: string;
  readonly protocol = "prns.transport.v1";
  readonly gatewayId: string;
  binaryType: BinaryType = "blob";
  bufferedAmount = 0;
  readyState = 0;
  #helloSent = false;

  constructor(url: string | URL, _protocols?: string | string[]) {
    super();
    this.url = url.toString();
    this.gatewayId = gatewayIdForUrl(this.url);
    GatewayWebSocket.instances.push(this);
    queueMicrotask(() => {
      this.readyState = 1;
      this.dispatchEvent(new Event("open"));
    });
  }

  send(data: string | ArrayBufferLike | Blob | ArrayBufferView): void {
    if (!this.#helloSent) {
      this.#helloSent = true;
      assert(sendBytes(data).byteLength === 10, "client hello is bounded");
      queueMicrotask(() => {
        this.emitMessage(serverHello(this.gatewayId));
        queueMicrotask(() => this.emitMessage(new Uint8Array([0xa5])));
      });
    }
  }

  close(): void {
    if (this.readyState === 3) {
      return;
    }
    this.readyState = 3;
    this.dispatchEvent(new Event("close"));
  }

  emitMessage(data: Uint8Array): void {
    const event = new Event("message") as MessageEvent;
    Object.defineProperty(event, "data", { value: data.buffer });
    this.dispatchEvent(event);
  }
}

class DeniedWebSocket {
  constructor() {
    throw new DOMException("local network denied", "NotAllowedError");
  }
}

async function main(): Promise<void> {
  validateCatalogBoundaries();
  const host = globalThis as typeof globalThis & {
    WebSocket?: typeof WebSocket;
    localStorage?: Storage;
  };
  const websocket = Object.getOwnPropertyDescriptor(globalThis, "WebSocket");
  const fetch = Object.getOwnPropertyDescriptor(globalThis, "fetch");
  const localStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(host, "localStorage", {
    configurable: true,
    value: new MemoryStorage(),
  });
  Object.defineProperty(host, "WebSocket", {
    configurable: true,
    value: GatewayWebSocket,
  });
  Object.defineProperty(host, "fetch", {
    configurable: true,
    value: catalogFetch,
  });

  try {
    const prns = await readyPrns();
    assert(GatewayWebSocket.instances.length === 0, "Prns.create has no network side effects");
    const controller = prns.interfaces.autoWifi.start();
    assert(
      controller === prns.interfaces.autoWifi.start(),
      "Auto Wi-Fi startup is idempotent while active",
    );
    const active = await waitForActive(controller);
    assert(active.data.gateways.length === 3, "at most three gateway stars attach");
    assert(active.data.gateways[0]?.id === LOCAL_ID, "localhost is retained first");
    assert(
      new Set(active.data.gateways.map((gateway) => gateway.id)).size === 3,
      "hello IDs deduplicate direct and catalog results",
    );
    await waitFor(
      () => (MockRuntime.latest?.ingested.length ?? 0) >= 3,
      1_000,
      "transport frames sent immediately after the hello",
    );
    const stableIds = active.data.gateways.map((gateway) => gateway.id).sort();
    const disconnected = active.data.gateways.find((gateway) => !gateway.localhost);
    assert(disconnected, "a remote gateway was selected");
    const socket = [...GatewayWebSocket.instances]
      .reverse()
      .find((candidate) => candidate.url === disconnected.url && candidate.readyState === 1);
    assert(socket, "selected remote socket exists");
    socket.emitMessage(new Uint8Array(573));
    const failedOver = await waitForActive(
      controller,
      (gateways) =>
        gateways.length === 3 &&
        !gateways.some((gateway) => gateway.id === disconnected.id),
      3_000,
    );
    assert(
      failedOver.data.gateways.length === 3,
      "an oversized frame disconnects the gateway and promotes the next rank",
    );
    await controller.close();

    const restarted = prns.interfaces.autoWifi.start();
    const restartedActive = await waitForActive(restarted);
    assert(
      restartedActive.data.gateways.map((gateway) => gateway.id).sort().join(",") ===
        stableIds.join(","),
      "persisted selection seed keeps gateway choice stable",
    );
    await restarted.close();

    Object.defineProperty(host, "WebSocket", {
      configurable: true,
      value: DeniedWebSocket,
    });
    Object.defineProperty(host, "fetch", {
      configurable: true,
      value: async () => {
        throw new DOMException("local network denied", "NotAllowedError");
      },
    });
    const denied = (await readyPrns()).interfaces.autoWifi.start();
    await waitFor(
      () => denied.status.tag === "Unavailable",
      1_000,
      "permission denial status",
    );
    assert(
      denied.status.tag === "Unavailable" && denied.status.data.tag === "PermissionDenied",
      "Local Network Access denial is typed",
    );
    await denied.close();
  } finally {
    restoreProperty(host, "WebSocket", websocket);
    restoreProperty(host, "fetch", fetch);
    restoreProperty(host, "localStorage", localStorage);
  }
}

function validateCatalogBoundaries(): void {
  const valid = encode({
    version: 1,
    gateways: [{ id: ALIAS_ID, url: "ws://192.168.4.2:42721/prns" }],
  });
  assert(parseBrowserGatewayCatalog(valid).tag === "Discovered", "typed catalog parses");
  for (const value of [
    { version: 1, gateways: [{ id: ALIAS_ID, url: "ws://8.8.8.8:42721/prns" }] },
    { version: 1, gateways: [{ id: ALIAS_ID, url: "ws://user@192.168.4.2:42721/prns" }] },
    { version: 1, gateways: [{ id: ALIAS_ID, url: "wss://192.168.4.2:42721/prns" }] },
    { version: 1, gateways: [{ id: ALIAS_ID, url: "ws://192.168.4.2:42721/prns?redirect=1" }] },
    { version: 1, gateways: [{ id: ALIAS_ID, url: "ws://-deceptive.local:42721/prns" }] },
    { version: 1, gateways: [{ id: ALIAS_ID, url: "ws://192.168.4.2:42721/prns" }], extra: true },
    { version: 1, gateways: [{ id: ALIAS_ID, url: "ws://192.168.4.2:42721/prns" }, { id: ALIAS_ID, url: "ws://192.168.4.3:42721/prns" }] },
  ]) {
    assert(parseBrowserGatewayCatalog(encode(value)).tag === "Failed", "hostile catalog fails");
  }
  assert(
    validateBrowserGatewayUrl("ws://prns.local:42721/prns").tag === "Valid",
    ".local transport URL is valid",
  );
  assert(
    validateBrowserGatewayUrl("ws://100.64.0.1:42721/prns").tag === "Invalid",
    "shared address space is excluded",
  );
  assert(
    parseBrowserGatewayCatalog(new Uint8Array(16 * 1024 + 1)).tag === "Failed",
    "oversized catalogs are rejected before parsing",
  );
}

async function readyPrns(): Promise<Prns> {
  const outcome = await Prns.create({
    wasm: wasmModule(),
    identityStore: {
      load: async () => Tag("Loaded", identitySecretKey(new Uint8Array(IDENTITY_LENGTH).fill(7), IDENTITY_LENGTH)),
      save: async () => Tag("Saved"),
    },
    bleIdentityStore: fixedBleIdentityStore(),
    entropy: (length) => Tag("Filled", entropyBytes(new Uint8Array(Math.max(length, 64)).fill(9))),
    now: () => nowMillis(123_456),
  });
  assert(outcome.tag === "Ready", `Prns is ready, got ${outcome.tag}`);
  return outcome.data;
}

function fixedBleIdentityStore(): StableIdentityStore {
  return {
    load: async () => Tag("Loaded", new Uint8Array(BLE_IDENTITY_LENGTH).fill(8)),
    save: async () => Tag("Saved"),
  };
}

function wasmModule(): PrnsWasmModule {
  return {
    PrnsRuntime: MockRuntime,
    UsbAutoDecoder: MockUsbAutoDecoder,
    BluetoothReassembler: MockBluetoothReassembler,
    identitySecretKeyLength: () => IDENTITY_LENGTH,
    bluetoothServiceUuid: () => "service",
    bluetoothControlUuid: () => "control",
    bluetoothDataUuid: () => "data",
    bluetoothBitrateBps: () => 125_000,
    bluetoothHardwareMtu: () => 508,
    bluetoothDialerHello: () => new Uint8Array([1]),
    bluetoothDecodeControl: () => ({ type: "close", reason: "unused" }),
    bluetoothDataFragments: () => [],
    websocketBitrateBps: () => 1_000_000_000,
    websocketFrameCap: () => 572,
    websocketHardwareMtu: () => 508,
    usbAutoHostBitrateBps: () => 1_000_000,
    usbAutoHostHardwareMtu: () => 508,
    usbAutoWebUsbVendorId: () => 1,
    usbAutoWebUsbProductId: () => 2,
    usbAutoNodeTagFor: () => new Uint8Array([1]),
    usbAutoHostHelloFrame: () => new Uint8Array([1]),
    usbAutoHostHelloAckFrame: () => new Uint8Array([1]),
    usbAutoDataFrame: () => new Uint8Array([1]),
  };
}

async function catalogFetch(
  _input: RequestInfo | URL,
  init?: RequestInit & { targetAddressSpace?: string },
): Promise<Response> {
  assert(init?.redirect === "error", "catalog redirects are disabled");
  assert(init.credentials === "omit", "catalog requests omit credentials");
  assert(
    init.targetAddressSpace === "loopback" || init.targetAddressSpace === "local",
    "catalog requests explicitly target local address space",
  );
  const gateways = REMOTE_IDS.map((id, index) => ({
    id,
    url: `ws://192.168.4.${index + 2}:42721/prns`,
  }));
  return new Response(JSON.stringify({ version: 1, gateways }), {
    headers: { "Content-Type": "application/json" },
    status: 200,
  });
}

async function waitForActive(
  controller: ReturnType<Prns["interfaces"]["autoWifi"]["start"]>,
  predicate: (gateways: readonly { id: BrowserRendezvousId }[]) => boolean = () => true,
  timeoutMs = 1_000,
) {
  await waitFor(
    () => controller.status.tag === "Active" && predicate(controller.status.data.gateways),
    timeoutMs,
    "active Auto Wi-Fi status",
  );
  const status = controller.status;
  assert(status.tag === "Active", "Auto Wi-Fi is active");
  return status;
}

async function waitFor(
  predicate: () => boolean,
  timeoutMs: number,
  label: string,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() >= deadline) {
      throw new Error(`timed out waiting for ${label}`);
    }
    await new Promise((resolve) => globalThis.setTimeout(resolve, 5));
  }
}

function gatewayIdForUrl(url: string): string {
  const host = new URL(url).hostname;
  if (host === "localhost") {
    return LOCAL_ID;
  }
  if (host === "prns.local") {
    return ALIAS_ID;
  }
  const octet = Number(host.split(".").at(-1));
  const id = REMOTE_IDS[octet - 2];
  assert(id, `known gateway URL ${url}`);
  return id;
}

function serverHello(id: string): Uint8Array {
  const bytes = new Uint8Array(26);
  bytes.set([0x50, 0x52, 0x4e, 0x53, 0x57, 0x53, 0, 0, 0, 1]);
  for (let index = 0; index < 16; index += 1) {
    bytes[index + 10] = Number.parseInt(id.slice(index * 2, index * 2 + 2), 16);
  }
  return bytes;
}

function sendBytes(value: string | ArrayBufferLike | Blob | ArrayBufferView): Uint8Array {
  if (typeof value === "string" || value instanceof Blob) {
    throw new Error("binary frame expected");
  }
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  return new Uint8Array(value);
}

function encode(value: unknown): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(value));
}

function restoreProperty(
  target: object,
  key: PropertyKey,
  descriptor: PropertyDescriptor | undefined,
): void {
  if (descriptor) {
    Object.defineProperty(target, key, descriptor);
  } else {
    Reflect.deleteProperty(target, key);
  }
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

await main();
