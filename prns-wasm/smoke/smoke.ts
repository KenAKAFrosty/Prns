import init, {
  BluetoothReassembler,
  PrnsRuntime,
  UsbAutoDecoder,
  WebSocketFramingCodec,
  bluetoothBitrateBps,
  bluetoothControlUuid,
  bluetoothDataFragments,
  bluetoothDataUuid,
  bluetoothDecodeControl,
  bluetoothDialerHello,
  bluetoothHardwareMtu,
  bluetoothServiceUuid,
  compressResourceCandidate,
  browserPersistenceVersion,
  hostContractAbi,
  hostSchemaVersion,
  identitySecretKeyLength,
  productVersion,
  websocketBitrateBps,
  websocketFrameCap,
  websocketHardwareMtu,
  usbAutoDataFrame,
  usbAutoHostBitrateBps,
  usbAutoHostHardwareMtu,
  usbAutoHostHelloAckFrame,
  usbAutoHostHelloFrame,
  usbAutoNodeTagFor,
  usbAutoWebUsbProductId,
  usbAutoWebUsbVendorId,
} from "/pkg/prns_wasm.js";
import {
  BLE_IDENTITY_LENGTH,
  Prns,
  appData,
  appName,
  aspect,
  bitrateBps,
  channelTag,
  entropyBytes,
  hardwareMtu,
  identitySecretKey,
  match,
  match_into,
  nowMillis,
  packetFrame,
} from "../../prns-js/src/browser/index.js";
import type {
  DestinationHash,
  InterfaceSnapshot,
  PrnsRuntimeBinding,
  PrnsEvent,
  PrnsSnapshot,
  PrnsWasmModule,
  RuntimeRegisterInterfaceInput,
  UsbAutoSession,
} from "../../prns-js/src/browser/index.js";

const wasmUrl = new URL("../../pkg/prns_wasm_bg.wasm", import.meta.url);

const runtimeStatus = element("runtime");
const usbStatus = element("usb");
const snapshotStatus = element("snapshot");
const interfacesStatus = element("interfaces");
const logView = element("status");
const connectButton = button("connect");
const announceButton = button("announce");
const closeButton = button("close");

type RuntimeOutbound = {
  bytes: Uint8Array;
};

let prns: Prns | undefined;
let session: UsbAutoSession | undefined;
let destination: DestinationHash | undefined;
let eventCount = 0;

function element(id: string): HTMLElement {
  const found = document.getElementById(id);
  assert(found instanceof HTMLElement, `${id} element exists`);
  return found;
}

function button(id: string): HTMLButtonElement {
  const found = document.getElementById(id);
  assert(found instanceof HTMLButtonElement, `${id} button exists`);
  return found;
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

function log(line: string): void {
  const now = new Date().toLocaleTimeString();
  logView.textContent = `${logView.textContent ?? ""}${now}  ${line}\n`;
  logView.scrollTop = logView.scrollHeight;
}

function entropy(length: number): Uint8Array {
  const bytes = new Uint8Array(length);
  crypto.getRandomValues(bytes);
  return bytes;
}

function runtimeOutbound(raw: unknown): RuntimeOutbound {
  assert(typeof raw === "object" && raw !== null, "outbound frame is object");
  const maybeFrame = raw as Partial<RuntimeOutbound>;
  assert(maybeFrame.bytes instanceof Uint8Array, "outbound bytes are Uint8Array");
  return { bytes: maybeFrame.bytes };
}

async function runRuntimeSmoke(): Promise<void> {
  const compressed = compressResourceCandidate({
    payload: new Uint8Array(4096).fill(7),
  });
  assert(
    compressed !== undefined && compressed.length < 4096,
    "resource compressor keeps a smaller candidate",
  );
  const identityLength = identitySecretKeyLength();
  const runtime: PrnsRuntimeBinding = new PrnsRuntime(
    identitySecretKey(entropy(identityLength), identityLength),
    entropy(BLE_IDENTITY_LENGTH),
  );
  const maximumSegmentBytes = 1024 * 1024 - 1;
  const boundaryPlan = runtime.resourceSegmentPlan({
    totalDataBytes: maximumSegmentBytes * 2 + 10,
    segmentIndex: 2,
  });
  assert(
    typeof boundaryPlan === "object" &&
      boundaryPlan !== null &&
      "type" in boundaryPlan &&
      boundaryPlan.type === "ready",
    "resource boundary plan is ready",
  );
  assert(
    "totalSegments" in boundaryPlan &&
      boundaryPlan.totalSegments === 3 &&
      "dataStart" in boundaryPlan &&
      boundaryPlan.dataStart === maximumSegmentBytes &&
      "dataEnd" in boundaryPlan &&
      typeof boundaryPlan.dataEnd === "number" &&
      boundaryPlan.dataEnd > boundaryPlan.dataStart,
    "resource boundary plan balances the tail",
  );
  const rejectedPlan = runtime.resourceSegmentPlan({
    totalDataBytes: 1,
    segmentIndex: 1,
    packedMetadataBytes: maximumSegmentBytes + 1,
  });
  assert(
    typeof rejectedPlan === "object" &&
      rejectedPlan !== null &&
      "type" in rejectedPlan &&
      rejectedPlan.type === "rejected" &&
      "cause" in rejectedPlan &&
      rejectedPlan.cause === "metadataTooLarge",
    "resource metadata rejection stays typed",
  );

  const interfaceOptions: RuntimeRegisterInterfaceInput = {
    kind: "auto-usb-host",
    channelTag: channelTag(new TextEncoder().encode("browser-smoke:usb")),
    bitrateBps: bitrateBps(usbAutoHostBitrateBps()),
    hardwareMtu: hardwareMtu(usbAutoHostHardwareMtu()),
    nowMs: nowMillis(),
  };
  const interfaceId = runtime.registerInterface(interfaceOptions);

  const smokeDestination = runtime.registerSingleDestination({
    appName: appName("prns"),
    aspects: [aspect("browser"), aspect("smoke")],
    appData: appData(),
  });

  const commandId = runtime.announce({
    destination: smokeDestination,
    nowMs: nowMillis(),
    entropy: entropyBytes(entropy(128)),
  });
  assert(typeof commandId === "bigint", "command id is bigint");

  const outbound = runtime.drainOutbound();
  assert(outbound.length > 0, "announce emits outbound frame");
  const firstOutbound = outbound[0];
  assert(firstOutbound !== undefined, "first outbound frame exists");
  const firstFrame = runtimeOutbound(firstOutbound);

  runtime.ingest({
    interfaceId,
    bytes: packetFrame(firstFrame.bytes),
    nowMs: nowMillis(),
    entropy: entropyBytes(entropy(128)),
  });

  const events = runtime.drainEvents();
  assert(
    events.some(
      (event) =>
        typeof event === "object" &&
        event !== null &&
        "type" in event &&
        event.type === "commandSettled",
    ),
    "announce command settles",
  );

  const snapshot = runtime.snapshot();
  assert(typeof snapshot === "object" && snapshot !== null, "snapshot is object");
  assert("type" in snapshot && snapshot.type === "snapshot", "snapshot has type");
  assert(
    "ingestedPackets" in snapshot &&
      typeof snapshot.ingestedPackets === "number" &&
      snapshot.ingestedPackets >= 1,
    "snapshot counted ingested packet",
  );

  runtimeStatus.textContent = `PASS outbound=${outbound.length} events=${events.length} packets=${snapshot.ingestedPackets}`;
  log(`runtime smoke passed: outbound=${outbound.length}, events=${events.length}`);
}

function wasmModule(): PrnsWasmModule {
  return {
    PrnsRuntime: PrnsRuntime as PrnsWasmModule["PrnsRuntime"],
    UsbAutoDecoder: UsbAutoDecoder as PrnsWasmModule["UsbAutoDecoder"],
    BluetoothReassembler:
      BluetoothReassembler as PrnsWasmModule["BluetoothReassembler"],
    WebSocketFramingCodec:
      WebSocketFramingCodec as PrnsWasmModule["WebSocketFramingCodec"],
    hostContractAbi,
    hostSchemaVersion,
    browserPersistenceVersion,
    productVersion,
    identitySecretKeyLength,
    bluetoothServiceUuid,
    bluetoothControlUuid,
    bluetoothDataUuid,
    bluetoothBitrateBps,
    bluetoothHardwareMtu,
    bluetoothDialerHello,
    bluetoothDecodeControl,
    bluetoothDataFragments,
    websocketBitrateBps,
    websocketFrameCap,
    websocketHardwareMtu,
    usbAutoHostBitrateBps,
    usbAutoHostHardwareMtu,
    usbAutoWebUsbVendorId,
    usbAutoWebUsbProductId,
    usbAutoNodeTagFor,
    usbAutoHostHelloFrame,
    usbAutoHostHelloAckFrame,
    usbAutoDataFrame,
  };
}

async function connectUsb(): Promise<void> {
  assert(prns, "Prns is ready");
  connectButton.disabled = true;
  usbStatus.textContent = "requesting browser USB device";
  log("requesting USB device");
  const connected = await prns.interfaces.usbAuto.connect();
  if (connected.tag !== "Connected") {
    usbStatus.textContent = "connect failed";
    connectButton.disabled = false;
    log(`${connected.tag}: ${JSON.stringify(connected.data)}`);
    return;
  }
  session = connected.data;
  usbStatus.textContent = describeSession(session);
  announceButton.disabled = false;
  closeButton.disabled = false;
  log(`USB Auto opened: interface=${hex(session.interfaceId)}`);
}

async function sendAnnounce(): Promise<void> {
  assert(prns, "Prns is ready");
  assert(destination, "destination is registered");
  const command = await prns.announce(destination);
  if (command.tag === "Failed") {
    log(`announce failed: ${command.data.tag}`);
    return;
  }
  log(`${command.data.tag} settled`);
}

async function consumeEvents(node: Prns): Promise<void> {
  const claim = node.claimEvents();
  if (claim.tag === "AlreadyClaimed") {
    log(`${claim.data.lane} already has a consumer`);
    return;
  }
  for await (const event of claim.data) {
    eventCount += 1;
    log(`event ${eventCount}: ${describeEvent(event)}`);
  }
}

async function consumeDiagnostics(node: Prns): Promise<void> {
  const claim = node.claimDiagnostics();
  if (claim.tag === "AlreadyClaimed") {
    log(`${claim.data.lane} already has a consumer`);
    return;
  }
  for await (const event of claim.data) {
    eventCount += 1;
    log(`diagnostic ${eventCount}: ${describeEvent(event)}`);
  }
}

async function closeUsb(): Promise<void> {
  await session?.close();
  session = undefined;
  usbStatus.textContent = "closed";
  connectButton.disabled = false;
  announceButton.disabled = true;
  closeButton.disabled = true;
  log("USB session closed");
}

function pollRuntime(): void {
  if (!prns) {
    return;
  }
  if (session) {
    usbStatus.textContent = describeSession(session);
    if (session.status.tag === "Failed" || session.status.tag === "Closed") {
      connectButton.disabled = false;
      closeButton.disabled = true;
      announceButton.disabled = true;
    }
  }
  const captured = prns.snapshot();
  if (captured.tag !== "Captured") {
    snapshotStatus.textContent = captured.tag;
    return;
  }
  const snapshot = captured.data;
  snapshotStatus.textContent = describeSnapshot(snapshot);
  interfacesStatus.textContent =
    snapshot.interfaces.map(describeInterface).join("\n") || "none";
}

function describeSession(value: UsbAutoSession): string {
  const base = `${value.status.tag} interface=${hex(value.interfaceId)}`;
  return value.status.tag === "Failed"
    ? `${base} failure=${value.status.data.tag}`
    : base;
}

function describeSnapshot(snapshot: PrnsSnapshot): string {
  return (
    `interfaces=${snapshot.interfaces.length} routes=${snapshot.routes} ` +
    `packets=${snapshot.ingestedPackets} commands=${snapshot.ingestedCommands} ` +
    `events=${eventCount}`
  );
}

function describeInterface(snapshot: InterfaceSnapshot): string {
  const bitrate = snapshot.bitrateBps ? ` bitrate=${snapshot.bitrateBps}` : "";
  const mtu = snapshot.hardwareMtu ? ` mtu=${snapshot.hardwareMtu}` : "";
  return (
    `${hex(snapshot.id)} ${snapshot.kind}` +
    ` routes=${snapshot.routes} links=${snapshot.links}${bitrate}${mtu}`
  );
}

function describeEvent(event: PrnsEvent): string {
  return match_into<string>().from<PrnsEvent>(event, {
    AnnounceHeard: ({ destination, hops, sourceInterface }) =>
      `announce destination=${hex(destination)} hops=${hops} interface=${hex(sourceInterface)}`,
    SingleDelivery: ({ destination, plaintext, sourceInterface }) =>
      `single delivery destination=${hex(destination)} bytes=${plaintext.length} interface=${hex(sourceInterface)}`,
    LinkDelivery: ({ linkId, plaintext, sourceInterface }) =>
      `link delivery link=${hex(linkId)} bytes=${plaintext.length} interface=${hex(sourceInterface)}`,
    Request: ({ destination, linkId, requestId, data }) =>
      `request destination=${hex(destination)} link=${hex(linkId)} request=${hex(requestId)} bytes=${data.length}`,
    Response: ({ linkId, requestId, data }) =>
      `response link=${hex(linkId)} request=${hex(requestId)} bytes=${data.length}`,
    ResponseSegment: ({
      linkId,
      requestId,
      segmentIndex,
      totalSegments,
      data,
    }) =>
      `response segment link=${hex(linkId)} request=${hex(requestId)} segment=${segmentIndex + 1}/${totalSegments} bytes=${data.length}`,
    ResourceAvailable: ({ linkId, hash, resource }) =>
      `resource link=${hex(linkId)} hash=${hex(hash)} bytes=${resource.totalBytes}`,
    ResourceSegment: ({
      linkId,
      originalHash,
      segmentIndex,
      totalSegments,
      data,
    }) =>
      `resource segment link=${hex(linkId)} hash=${hex(originalHash)} segment=${segmentIndex + 1}/${totalSegments} bytes=${data.length}`,
    ChannelMessage: ({ linkId, messageType, data }) =>
      `channel link=${hex(linkId)} type=${messageType} bytes=${data.length}`,
    LinkEstablished: ({ linkId, rttMillis }) =>
      `link established=${hex(linkId)} rtt=${rttMillis}ms`,
    PeerIdentified: ({ linkId, identity }) =>
      `peer identified link=${hex(linkId)} identity=${hex(identity)}`,
    LinkClosed: ({ linkId, reason }) =>
      `link closed=${hex(linkId)} reason=${reason}`,
    LinkInterfaceMismatch: ({ linkId, attachedInterface, arrivedOn }) =>
      `link mismatch=${hex(linkId)} attached=${hex(attachedInterface)} arrived=${hex(arrivedOn)}`,
    ResourceNeedsDecompression: ({
      linkId,
      hash,
      stream,
      uncompressedDataBytes,
    }) =>
      `resource compressed link=${hex(linkId)} hash=${hex(hash)} compressed=${stream.length} uncompressed=${uncompressedDataBytes}`,
    ResourceAssembled: ({ linkId, originalHash, totalSizeBytes }) =>
      `resource assembled link=${hex(linkId)} hash=${hex(originalHash)} bytes=${totalSizeBytes}`,
    ResourceFailed: ({ linkId, hash, cause }) =>
      `resource failed link=${hex(linkId)} hash=${hex(hash)} cause=${cause}`,
    ResourceSendProgress: ({
      linkId,
      transferredBytes,
      totalBytes,
      physicalTransferredBytes,
      segmentIndex,
      totalSegments,
    }) =>
      `resource progress link=${hex(linkId)} bytes=${transferredBytes}/${totalBytes} physical=${physicalTransferredBytes} segment=${segmentIndex + 1}/${totalSegments}`,
    SelfRatchetRotated: ({ destination }) =>
      `self ratchet rotated destination=${hex(destination)}`,
    AnnounceHeldDropped: ({ destination, sourceInterface, cause }) =>
      `announce held dropped destination=${hex(destination)} interface=${hex(sourceInterface)} cause=${cause}`,
    Delivered: ({ detail }) => `delivered ${detail}`,
    BackendDiagnostic: ({ kind, detail }) => `${kind}: ${detail}`,
    RouteExpired: ({ destination }) =>
      `RouteExpired destination=${hex(destination)}`,
    RouteEvicted: ({ destination }) =>
      `RouteEvicted destination=${hex(destination)}`,
    RouteInterfaceGone: ({ destination }) =>
      `RouteInterfaceGone destination=${hex(destination)}`,
    RouteDropped: ({ destination }) =>
      `RouteDropped destination=${hex(destination)}`,
    DiagnosticsDropped: ({ count }) =>
      `diagnostics dropped=${count.toString()}`,
    PersistenceRestored: ({
      routes,
      destinationIdentities,
      tunnels,
      ratchets,
      refused,
      dropped,
    }) =>
      `persistence restored routes=${routes} identities=${destinationIdentities} tunnels=${tunnels} ratchets=${ratchets} refused=${refused} dropped=${dropped}`,
    PersistenceFlushed: ({ cause, target }) =>
      `persistence flushed cause=${cause} target=${target}`,
    PersistenceFlushFailed: ({ cause, target }) =>
      `persistence flush failed cause=${cause} target=${target}`,
  });
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function describeError(error: unknown): string {
  if (error instanceof DOMException) {
    return `${error.name}: ${error.message}`;
  }
  if (error instanceof Error) {
    return error.stack ?? `${error.name}: ${error.message}`;
  }
  return String(error);
}

try {
  logView.textContent = "";
  await init(wasmUrl);
  await runRuntimeSmoke();

  const created = await Prns.create({ wasm: wasmModule() });
  assert(created.tag === "Ready", `Prns creation failed: ${created.tag}`);
  prns = created.data;
  void consumeEvents(prns);
  void consumeDiagnostics(prns);
  const registered = prns.registerSingleDestination({
    appName: appName("prns"),
    aspects: [aspect("browser"), aspect("playground")],
    appData: appData(),
  });
  assert(
    registered.tag === "Registered",
    `destination registration failed: ${registered.tag}`,
  );
  destination = registered.data;
  connectButton.disabled = !("usb" in navigator);
  usbStatus.textContent = connectButton.disabled
    ? "WebUSB unavailable in this browser"
    : "ready";
  log(`registered browser playground destination: ${hex(destination)}`);
  window.setInterval(pollRuntime, 250);
  document.title = "PASS";
} catch (error: unknown) {
  console.error(error);
  runtimeStatus.textContent = "FAIL";
  log(describeError(error));
  document.title = "FAIL";
}

connectButton.addEventListener("click", () => {
  void connectUsb();
});
announceButton.addEventListener("click", () => {
  void sendAnnounce();
});
closeButton.addEventListener("click", () => {
  void closeUsb();
});
