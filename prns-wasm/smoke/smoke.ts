import init, {
  BluetoothReassembler,
  PrnsRuntime,
  UsbAutoDecoder,
  bluetoothBitrateBps,
  bluetoothControlUuid,
  bluetoothDataFragments,
  bluetoothDataUuid,
  bluetoothDecodeControl,
  bluetoothDialerHello,
  bluetoothHardwareMtu,
  bluetoothServiceUuid,
  identitySecretKeyLength,
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
  nowMillis,
  packetFrame,
} from "../ts/index.js";
import type {
  DestinationHash,
  InterfaceSnapshot,
  PrnsRuntimeBinding,
  PrnsEvent,
  PrnsSnapshot,
  PrnsWasmModule,
  RuntimeRegisterInterfaceInput,
  UsbAutoSession,
} from "../ts/index.js";

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
  const identityLength = identitySecretKeyLength();
  const runtime: PrnsRuntimeBinding = new PrnsRuntime(
    identitySecretKey(entropy(identityLength), identityLength),
    entropy(BLE_IDENTITY_LENGTH),
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
    entropy: entropyBytes(entropy(64)),
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
    entropy: entropyBytes(entropy(64)),
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

function sendAnnounce(): void {
  assert(prns, "Prns is ready");
  assert(destination, "destination is registered");
  const command = prns.announce(destination);
  log(
    command.tag === "Queued"
      ? `announce queued: command=${command.data.toString()}`
      : `${command.tag}: ${JSON.stringify(command.data)}`,
  );
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
  const drained = prns.drainEvents();
  if (drained.tag === "Drained") {
    for (const event of drained.data) {
      eventCount += 1;
      log(`event ${eventCount}: ${describeEvent(event)}`);
    }
  } else {
    log(`${drained.tag}: ${JSON.stringify(drained.data)}`);
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
  switch (event.type) {
    case "announce":
      return `announce destination=${hex(event.destination)} hops=${event.hops} interface=${hex(event.sourceInterface)}`;
    case "commandSettled":
      return `command settled id=${event.commandId.toString()} ${event.debugSettlement}`;
    case "routeExpired":
    case "routeEvicted":
    case "routeInterfaceGone":
    case "routeDropped":
      return `${event.type} destination=${hex(event.destination)}`;
    case "unknown":
      return `unknown ${JSON.stringify(event.raw)}`;
  }
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
announceButton.addEventListener("click", sendAnnounce);
closeButton.addEventListener("click", () => {
  void closeUsb();
});
