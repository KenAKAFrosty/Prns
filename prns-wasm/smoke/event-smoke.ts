import {
  BLE_IDENTITY_LENGTH,
  Prns,
  PRODUCT_VERSION,
  Tag,
  destinationHash,
  entropyBytes,
  identitySecretKey,
  interfaceId,
  linkId,
  nowMillis,
  match_into,
} from "../../prns-js/src/browser/index.js";
import type {
  BleIdentity,
  BluetoothSession,
  BluetoothReassemblerBinding,
  DestinationHash,
  IdentitySecretKey,
  InterfaceId,
  InterfaceSessionStatus,
  PrnsRuntimeBinding,
  PrnsWasmModule,
  RuntimeAnnounceOptions,
  RuntimeIngestOptions,
  RuntimeRegisterInterfaceInput,
  RuntimeRegisterNodePageOptions,
  RuntimeRegisterSingleDestinationOptions,
  RuntimeRemoveInterfaceInput,
  StableIdentityStore,
  StreamClaim,
  UsbAutoDecoderBinding,
} from "../../prns-js/src/browser/index.js";
import type { PacketContentPresentation } from "../examples/browser-playground/presentation.js";
import {
  bluetoothClosableSession,
  bluetoothConnectAvailable,
  observeBluetoothSession,
} from "../examples/browser-playground/state.js";
import type { BluetoothState } from "../examples/browser-playground/state.js";
import {
  MockRuntimeBase,
  MockWebSocketFramingCodec,
} from "./mock_runtime.js";

const IDENTITY_LENGTH = 32;

class MockRuntime extends MockRuntimeBase {
  static latest: MockRuntime | undefined;
  readonly events: unknown[] = [];

  constructor(_identity: IdentitySecretKey, _bleIdentity?: BleIdentity) {
    super();
    MockRuntime.latest = this;
  }

  registerInterface(_options: RuntimeRegisterInterfaceInput): InterfaceId {
    return interfaceId(new Uint8Array(8).fill(1));
  }

  removeInterface(_options: RuntimeRemoveInterfaceInput): boolean {
    return true;
  }

  bluetoothIdentity(): Uint8Array {
    return new Uint8Array(BLE_IDENTITY_LENGTH).fill(2);
  }

  registerSingleDestination(
    _options: RuntimeRegisterSingleDestinationOptions,
  ): DestinationHash {
    return destinationHash(new Uint8Array(16).fill(3));
  }

  registerNodePage(_options: RuntimeRegisterNodePageOptions): DestinationHash {
    return destinationHash(new Uint8Array(16).fill(4));
  }

  announce(_options: RuntimeAnnounceOptions): bigint {
    this.events.push({
      type: "commandSettled",
      id: 1n,
      result: "succeeded",
      kind: "Announced",
    });
    return 1n;
  }

  sendSinglePacket(
    _options: Parameters<PrnsRuntimeBinding["sendSinglePacket"]>[0],
  ): bigint {
    this.events.push({
      type: "commandSettled",
      id: 2n,
      result: "succeeded",
      kind: "PacketDelivered",
      rttMillis: 7,
      evidence: "Response",
    });
    return 2n;
  }

  closeLink(
    _options: Parameters<PrnsRuntimeBinding["closeLink"]>[0],
  ): bigint {
    this.events.push({
      type: "commandSettled",
      id: 3n,
      result: "failed",
      kind: "UnknownLink",
    });
    return 3n;
  }

  ingest(_options: RuntimeIngestOptions): void {}

  drainEvents(): unknown[] {
    return this.events.splice(0);
  }

  drainOutbound(): unknown[] {
    return [];
  }

  snapshot(): unknown {
    return {
      type: "snapshot",
      ingestedPackets: 0,
      ingestedCommands: 0,
      routes: 0,
      scheduledAnnounces: 0,
      interfaces: [],
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

async function main(): Promise<void> {
  const prns = await readyPrns();
  const runtime = MockRuntime.latest;
  assert(runtime, "mock runtime exists");

  const destination = new Uint8Array(16).fill(4);
  const packet = await prns.execute(
    Tag("SendSinglePacket", {
      destination: destinationHash(destination),
      payload: new TextEncoder().encode("command payload"),
    }),
  );
  assert(packet.tag === "Succeeded", "typed command succeeds");
  assert(packet.data.tag === "PacketDelivered", "typed command outcome is preserved");
  const unsupported = await prns.execute(
    Tag("AttachTcpClient", {
      target: "127.0.0.1:4242",
      bitrate: Tag("Auto"),
    }),
  );
  assert(unsupported.tag === "Failed", "unsupported command is data");
  assert(
    unsupported.data.tag === "UnsupportedByBackend",
    "unsupported command failure is typed",
  );
  const closed = await prns.closeLink(linkId(new Uint8Array(16)));
  assert(closed.tag === "Failed", "link rejection is data");
  assert(closed.data.tag === "UnknownLink", "link rejection preserves its case");
  const sourceInterface = new Uint8Array(8).fill(5);
  const plaintext = new TextEncoder().encode("hello from a single packet");
  runtime.events.push({
    type: "singleDelivery",
    destination,
    plaintext,
    sourceInterface,
  });
  const events = claimed(prns.claimEvents());
  const delivered = await events.next();
  assert(!delivered.done, "single delivery streams");
  const event = delivered.value;
  assert(event.tag === "SingleDelivery", "single delivery is tagged");
  assert(bytesEqual(event.data.destination, destination), "destination is preserved");
  assert(
    bytesEqual(event.data.sourceInterface, sourceInterface),
    "source interface is preserved",
  );
  assert(bytesEqual(event.data.plaintext, plaintext), "plaintext is preserved");
  plaintext.fill(0);
  assert(
    new TextDecoder().decode(event.data.plaintext) === "hello from a single packet",
    "parsed plaintext owns its bytes",
  );

  const activeLink = new Uint8Array(16).fill(6);
  const linkPlaintext = new TextEncoder().encode("hello from a direct Link packet");
  runtime.events.push({
    type: "linkDelivery",
    linkId: activeLink,
    plaintext: linkPlaintext,
    sourceInterface,
  });
  const linkDelivered = await events.next();
  assert(!linkDelivered.done, "Link delivery streams");
  const linkEvent = linkDelivered.value;
  assert(linkEvent.tag === "LinkDelivery", "Link delivery is tagged");
  assert(bytesEqual(linkEvent.data.linkId, activeLink), "Link ID is preserved");
  assert(
    bytesEqual(linkEvent.data.sourceInterface, sourceInterface),
    "Link source interface is preserved",
  );
  assert(
    bytesEqual(linkEvent.data.plaintext, linkPlaintext),
    "Link plaintext is preserved",
  );
  linkPlaintext.fill(0);
  assert(
    new TextDecoder().decode(linkEvent.data.plaintext) ===
      "hello from a direct Link packet",
    "parsed Link plaintext owns its bytes",
  );

  const announceAppData = new Uint8Array([0, 112, 114, 110, 115, 255]);
  runtime.events.push(
    {
      type: "announce",
      appData: announceAppData,
      destination,
      hops: 2,
      sourceInterface,
    },
    { type: "commandSettled", id: 7n, result: "untracked" },
    { type: "routeExpired", destination },
  );
  const diagnostics = claimed(prns.claimDiagnostics());
  const announce = await diagnostics.next();
  const route = await diagnostics.next();
  assert(!announce.done && !route.done, "diagnostics stream");
  assert(
    `${announce.value.tag},${route.value.tag}` ===
      "AnnounceHeard,RouteExpired",
    "diagnostic cases are tagged and command settlement stays private",
  );
  assert(announce.value.tag === "AnnounceHeard", "announce diagnostic is tagged");
  assert(
    bytesEqual(announce.value.data.appData, announceAppData),
    "announce application data is preserved",
  );
  announceAppData.fill(0);
  assert(
    bytesEqual(
      announce.value.data.appData,
      new Uint8Array([0, 112, 114, 110, 115, 255]),
    ),
    "parsed announce application data owns its bytes",
  );

  runtime.events.push({ type: "futureEvent", value: 1 });
  assert(
    await rejects(diagnostics.next()),
    "unknown raw events are contract failures",
  );

  for (const malformed of [
    {
      type: "singleDelivery",
      destination: new Uint8Array(15),
      plaintext: new Uint8Array([1]),
      sourceInterface,
    },
    {
      type: "singleDelivery",
      destination,
      plaintext: "not bytes",
      sourceInterface,
    },
    {
      type: "singleDelivery",
      destination,
      plaintext: new Uint8Array([1]),
      sourceInterface: new Uint8Array(7),
    },
  ]) {
    const malformedPrns = await readyPrns();
    const malformedRuntime = MockRuntime.latest;
    assert(malformedRuntime, "malformed runtime exists");
    const malformedEvents = claimed(malformedPrns.claimEvents());
    malformedRuntime.events.push(malformed);
    assert(
      await rejects(malformedEvents.next()),
      "malformed single delivery is a typed drain failure",
    );
  }

  await validatePresentations();
  validateBluetoothState();
  console.log("event smoke passed");
}

function validateBluetoothState(): void {
  let status: InterfaceSessionStatus = Tag("Active");
  const session: BluetoothSession = {
    name: "bluetooth" as const,
    interfaceId: interfaceId(new Uint8Array(8).fill(31)),
    get status() {
      return status;
    },
    close: async () => Tag("Closed"),
  };
  const active: BluetoothState = Tag("Session", session);
  assert(
    !bluetoothConnectAvailable(active),
    "an active Bluetooth session cannot reconnect",
  );
  assert(
    bluetoothClosableSession(active) === session,
    "an active Bluetooth session can close",
  );
  status = Tag(
    "Failed",
    Tag("Disconnected", { detail: "fixture disconnected" }),
  );
  const observed = observeBluetoothSession(session);
  assert(
    observed.tag === "Failed" &&
      observed.data.tag === "Disconnected",
    "a failed Bluetooth session becomes an explicit failure observation",
  );
  assert(
    bluetoothConnectAvailable(active),
    "a failed Bluetooth session can reconnect without a close ceremony",
  );
  assert(
    bluetoothClosableSession(active) === undefined,
    "a failed Bluetooth session is not presented as closable",
  );
}

async function rejects(operation: Promise<unknown>): Promise<boolean> {
  try {
    await operation;
    return false;
  } catch {
    return true;
  }
}

function claimed<Value>(
  claim: StreamClaim<Value>,
): AsyncIterableIterator<Value> {
  return match_into<AsyncIterableIterator<Value>>().from(claim, {
    Claimed: (stream) => stream,
    AlreadyClaimed: ({ lane }) => fail(`${lane} was already claimed`),
  });
}

function fail(message: string): never {
  throw new Error(message);
}

async function validatePresentations(): Promise<void> {
  const presentationUrl = new URL(
    "../examples/browser-playground/presentation.js",
    import.meta.url,
  );
  const presentation: {
    describeBluetoothUnavailable(signals: {
      readonly platform?: string;
      readonly userAgent?: string;
      readonly userAgentData?: {
        readonly platform?: string;
        readonly brands?: readonly { readonly brand: string }[];
      };
    }): string;
    presentPacketContent(plaintext: Uint8Array): PacketContentPresentation;
  } = await import(presentationUrl.href);
  const text = presentation.presentPacketContent(
    new TextEncoder().encode("visible payload"),
  );
  assert(
    text.tag === "Text" && text.data.value === "visible payload",
    "UTF-8 payload is presented as text",
  );
  assert(
    presentation.presentPacketContent(new Uint8Array()).tag === "Empty",
    "empty payload has an explicit presentation",
  );
  const binary = presentation.presentPacketContent(new Uint8Array([0xff, 0x00]));
  assert(
    binary.tag === "Binary" &&
      binary.data.byteLength === 2 &&
      binary.data.hexadecimal === "ff00",
    "invalid UTF-8 payload is presented as bounded binary data",
  );
  const linuxChromium = presentation.describeBluetoothUnavailable({
    platform: "Linux x86_64",
    userAgent: "Mozilla/5.0 Chrome/151.0.0.0 Safari/537.36",
  });
  assert(
    linuxChromium.includes("--enable-experimental-web-platform-features"),
    "Linux Chromium receives actionable Web Bluetooth guidance",
  );
  const linuxFirefox = presentation.describeBluetoothUnavailable({
    platform: "Linux x86_64",
    userAgent: "Mozilla/5.0 Firefox/142.0",
  });
  assert(
    linuxFirefox === "Web Bluetooth is not exposed by this browser",
    "Linux Firefox does not receive Chromium-specific guidance",
  );
  const androidChromium = presentation.describeBluetoothUnavailable({
    platform: "Linux armv8l",
    userAgent: "Mozilla/5.0 (Linux; Android 16) Chrome/151.0.0.0",
  });
  assert(
    androidChromium === "Web Bluetooth is not exposed by this browser",
    "Android is not mistaken for desktop Linux",
  );
  const chromeOs = presentation.describeBluetoothUnavailable({
    platform: "Linux x86_64",
    userAgent: "Mozilla/5.0 (X11; CrOS x86_64 16093.68.0) Chrome/151.0.0.0",
  });
  assert(
    chromeOs === "Web Bluetooth is not exposed by this browser",
    "ChromeOS is not mistaken for desktop Linux",
  );
}

async function readyPrns(): Promise<Prns> {
  const outcome = await Prns.create({
    wasm: wasmModule(),
    identityStore: {
      load: async () =>
        Tag(
          "Loaded",
          identitySecretKey(
            new Uint8Array(IDENTITY_LENGTH).fill(6),
            IDENTITY_LENGTH,
          ),
        ),
      save: async () => Tag("Saved"),
    },
    bleIdentityStore: fixedBleIdentityStore(),
    entropy: (length) =>
      Tag(
        "Filled",
        entropyBytes(new Uint8Array(Math.max(length, 64)).fill(7)),
      ),
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
    WebSocketFramingCodec: MockWebSocketFramingCodec,
    hostContractAbi: () => 1,
    hostSchemaVersion: () => 1,
    browserPersistenceVersion: () => 1,
    productVersion: () => PRODUCT_VERSION,
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

function bytesEqual(left: Uint8Array, right: Uint8Array): boolean {
  return (
    left.length === right.length &&
    left.every((byte, index) => byte === right[index])
  );
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

await main();
