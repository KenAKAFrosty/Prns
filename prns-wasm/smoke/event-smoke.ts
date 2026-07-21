import {
  BLE_IDENTITY_LENGTH,
  Prns,
  Tag,
  destinationHash,
  entropyBytes,
  identitySecretKey,
  interfaceId,
  nowMillis,
} from "../ts/index.js";
import type {
  BleIdentity,
  BluetoothReassemblerBinding,
  DestinationHash,
  IdentitySecretKey,
  InterfaceId,
  PrnsRuntimeBinding,
  PrnsWasmModule,
  RuntimeAnnounceOptions,
  RuntimeIngestOptions,
  RuntimeRegisterInterfaceInput,
  RuntimeRegisterSingleDestinationOptions,
  RuntimeRemoveInterfaceInput,
  StableIdentityStore,
  UsbAutoDecoderBinding,
} from "../ts/index.js";
import type { PacketContentPresentation } from "../examples/browser-playground/presentation.js";

const IDENTITY_LENGTH = 32;

class MockRuntime implements PrnsRuntimeBinding {
  static latest: MockRuntime | undefined;
  readonly events: unknown[] = [];

  constructor(_identity: IdentitySecretKey, _bleIdentity?: BleIdentity) {
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

  announce(_options: RuntimeAnnounceOptions): bigint {
    return 1n;
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
  const sourceInterface = new Uint8Array(8).fill(5);
  const plaintext = new TextEncoder().encode("hello from a single packet");
  runtime.events.push({
    type: "singleDelivery",
    destination,
    plaintext,
    sourceInterface,
  });
  const delivered = prns.drainEvents();
  assert(delivered.tag === "Drained", "single delivery drains");
  assert(delivered.data.length === 1, "one single delivery drains");
  const event = delivered.data[0];
  assert(event?.type === "singleDelivery", "single delivery is typed");
  assert(bytesEqual(event.destination, destination), "destination is preserved");
  assert(
    bytesEqual(event.sourceInterface, sourceInterface),
    "source interface is preserved",
  );
  assert(bytesEqual(event.plaintext, plaintext), "plaintext is preserved");
  plaintext.fill(0);
  assert(
    new TextDecoder().decode(event.plaintext) === "hello from a single packet",
    "parsed plaintext owns its bytes",
  );

  runtime.events.push(
    {
      type: "announce",
      destination,
      hops: 2,
      sourceInterface,
    },
    { type: "commandSettled", id: 7n, settlement: "Sent" },
    { type: "routeExpired", destination },
    { type: "futureEvent", value: 1 },
  );
  const existing = prns.drainEvents();
  assert(existing.tag === "Drained", "existing events drain");
  assert(
    existing.data.map((candidate) => candidate.type).join(",") ===
      "announce,commandSettled,routeExpired,unknown",
    "existing event cases remain intact",
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
    runtime.events.push(malformed);
    const rejected = prns.drainEvents();
    assert(
      rejected.tag === "RuntimeRejected" &&
        rejected.data.operation === "drain-events",
      "malformed single delivery is a typed drain failure",
    );
  }

  await validatePresentations();
  console.log("event smoke passed");
}

async function validatePresentations(): Promise<void> {
  const presentationUrl = new URL(
    "../../../../docs/website/public/browser-node-playground-console/presentation.js",
    import.meta.url,
  );
  const presentation: {
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
