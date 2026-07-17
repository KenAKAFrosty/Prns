declare const brand: unique symbol;

type Brand<Name extends string> = { readonly [brand]: Name };
type BrandedBytes<Name extends string> = Uint8Array & Brand<Name>;
type BrandedNumber<Name extends string> = number & Brand<Name>;
type BrandedBigInt<Name extends string> = bigint & Brand<Name>;

export const INTERFACE_ID_LENGTH = 8;
export const DESTINATION_HASH_LENGTH = 16;
export const MIN_ENTROPY_BYTES = 64;

export type IdentitySecretKey = BrandedBytes<"IdentitySecretKey">;
export type InterfaceId = BrandedBytes<"InterfaceId">;
export type DestinationHash = BrandedBytes<"DestinationHash">;
export type ChannelTag = BrandedBytes<"ChannelTag">;
export type PacketFrame = BrandedBytes<"PacketFrame">;
export type EntropyBytes = BrandedBytes<"EntropyBytes">;
export type AppData = BrandedBytes<"AppData">;

export type AppName = string & Brand<"AppName">;
export type Aspect = string & Brand<"Aspect">;
export type InstantMillis = BrandedNumber<"InstantMillis">;
export type BitrateBps = BrandedNumber<"BitrateBps">;
export type HardwareMtu = BrandedNumber<"HardwareMtu">;
export type HopCount = BrandedNumber<"HopCount">;
export type CommandId = BrandedBigInt<"CommandId">;

export type PrnsValidationCode =
  | "empty-bytes"
  | "empty-string"
  | "disconnected"
  | "invalid-component"
  | "invalid-length"
  | "invalid-number"
  | "missing-host-api"
  | "operation-cancelled"
  | "permission-denied"
  | "transfer-failed"
  | "unsupported-frame"
  | "unknown-event"
  | "unknown-interface-kind"
  | "unknown-outbound-target";

export class PrnsValidationError extends Error {
  readonly code: PrnsValidationCode;

  constructor(code: PrnsValidationCode, message: string) {
    super(message);
    this.name = "PrnsValidationError";
    this.code = code;
  }
}

export type PrnsWasmModule = {
  PrnsRuntime: {
    new(identitySecretKey: IdentitySecretKey): PrnsRuntimeBinding;
  };
  UsbAutoDecoder: {
    new(): UsbAutoDecoderBinding;
  };
  BluetoothReassembler: {
    new(): BluetoothReassemblerBinding;
  };
  identitySecretKeyLength(): number;
  bluetoothServiceUuid(): string;
  bluetoothControlUuid(): string;
  bluetoothDataUuid(): string;
  bluetoothBitrateBps(): number;
  bluetoothHardwareMtu(): number;
  bluetoothDialerHello(identity: Uint8Array): Uint8Array;
  bluetoothDecodeControl(bytes: Uint8Array): unknown;
  bluetoothDataFragments(packet: PacketFrame): Uint8Array[];
  websocketBitrateBps(): number;
  websocketHardwareMtu(): number;
  usbAutoHostBitrateBps(): number;
  usbAutoHostHardwareMtu(): number;
  usbAutoWebUsbVendorId(): number;
  usbAutoWebUsbProductId(): number;
  usbAutoNodeTagFor(interfaceId: InterfaceId): Uint8Array;
  usbAutoHostHelloFrame(): Uint8Array;
  usbAutoHostHelloAckFrame(nodeTag: Uint8Array): Uint8Array;
  usbAutoDataFrame(packet: PacketFrame): Uint8Array;
};

export type PrnsRuntimeBinding = {
  registerInterface(options: RuntimeRegisterInterfaceOptions): InterfaceId;
  bluetoothIdentity(): Uint8Array;
  registerSingleDestination(options: RuntimeRegisterSingleDestinationOptions): DestinationHash;
  announce(options: RuntimeAnnounceOptions): bigint;
  ingest(options: RuntimeIngestOptions): void;
  drainEvents(): unknown[];
  drainOutbound(): unknown[];
  snapshot(): unknown;
};

export type UsbAutoDecoderBinding = {
  feed(chunk: Uint8Array): unknown[];
};

export type BluetoothReassemblerBinding = {
  absorb(bytes: Uint8Array): Uint8Array | undefined;
};

export type InterfaceName =
  | "usb-auto"
  | "rnode"
  | "bluetooth"
  | "websocket"
  | "serial"
  | "kiss"
  | "pipe";

export type RuntimeInterfaceKind =
  | "auto-usb-host"
  | "auto-usb-device"
  | "rnode"
  | "bluetooth-auto"
  | "bluetooth-peer"
  | "websocket-client"
  | "websocket-server"
  | "websocket-server-peer"
  | "serial"
  | "kiss"
  | "pipe";

export type RuntimeRegisterInterfaceOptions = {
  kind: RuntimeInterfaceKind;
  channelTag: ChannelTag;
  bitrateBps?: BitrateBps;
  hardwareMtu?: HardwareMtu;
};

export type RuntimeRegisterSingleDestinationOptions = {
  appName: AppName;
  aspects: readonly Aspect[];
  appData?: AppData;
};

export type RegisterSingleDestinationOptions =
  RuntimeRegisterSingleDestinationOptions;

export type RuntimeAnnounceOptions = {
  destination: DestinationHash;
  nowMs: InstantMillis;
  entropy: EntropyBytes;
};

export type RuntimeIngestOptions = {
  interfaceId: InterfaceId;
  bytes: PacketFrame;
  nowMs: InstantMillis;
  entropy: EntropyBytes;
};

export type AnnounceEvent = {
  type: "announce";
  destination: DestinationHash;
  hops: HopCount;
  sourceInterface: InterfaceId;
};

export type CommandSettledEvent = {
  type: "commandSettled";
  commandId: CommandId;
  debugSettlement: string;
};

export type RouteEvent = {
  type: "routeExpired" | "routeEvicted" | "routeInterfaceGone" | "routeDropped";
  destination: DestinationHash;
};

export type UnknownPrnsEvent = {
  type: "unknown";
  raw: unknown;
};

export type PrnsEvent =
  | AnnounceEvent
  | CommandSettledEvent
  | RouteEvent
  | UnknownPrnsEvent;

export type FanTarget =
  | { type: "all" }
  | { type: "only"; interfaceId: InterfaceId }
  | { type: "allExcept"; interfaceId: InterfaceId };

export type OutboundTarget =
  | { type: "interface"; interfaceId: InterfaceId }
  | { type: "broadcast"; supervisorKind: RuntimeInterfaceKind; fan: FanTarget };

export type PrnsOutboundFrame = {
  type: "frame" | "announce";
  target: OutboundTarget;
  hops?: HopCount;
  bytes: PacketFrame;
};

export type InterfaceSnapshot = {
  id: InterfaceId;
  kind: string;
  bitrateBps?: BitrateBps;
  hardwareMtu?: HardwareMtu;
  routes: number;
  links: number;
};

export type PrnsSnapshot = {
  type: "snapshot";
  ingestedPackets: number;
  ingestedCommands: number;
  routes: number;
  scheduledAnnounces: number;
  interfaces: InterfaceSnapshot[];
};

export type IdentityStore = {
  load(expectedLength: number): Promise<IdentitySecretKey | null>;
  save(secretKey: IdentitySecretKey): Promise<void>;
};

type HostGlobal = typeof globalThis & {
  crypto?: {
    getRandomValues<T extends Uint8Array>(array: T): T;
  };
  navigator?: {
    bluetooth?: BrowserBluetooth;
    serial?: BrowserSerial;
    usb?: BrowserUsb;
  };
  localStorage?: {
    getItem(key: string): string | null;
    setItem(key: string, value: string): void;
  };
  btoa?: (data: string) => string;
  atob?: (data: string) => string;
  WebSocket?: typeof WebSocket;
};

type BrowserBluetooth = {
  requestDevice(options: BrowserBluetoothRequestOptions): Promise<BrowserBluetoothDevice>;
};

type BrowserBluetoothRequestOptions = {
  filters: readonly BrowserBluetoothRequestFilter[];
  optionalServices?: readonly string[];
};

type BrowserBluetoothRequestFilter = {
  services: readonly string[];
};

type BrowserBluetoothDevice = {
  readonly gatt?: BrowserBluetoothRemoteGattServer;
};

type BrowserBluetoothRemoteGattServer = {
  connect(): Promise<BrowserBluetoothRemoteGattServer>;
  disconnect(): void;
  getPrimaryService(service: string): Promise<BrowserBluetoothRemoteGattService>;
};

type BrowserBluetoothRemoteGattService = {
  getCharacteristic(characteristic: string): Promise<BrowserBluetoothRemoteGattCharacteristic>;
};

type BrowserBluetoothRemoteGattCharacteristic = EventTarget & {
  readonly value?: DataView;
  startNotifications(): Promise<BrowserBluetoothRemoteGattCharacteristic>;
  writeValue?(value: BufferSource): Promise<void>;
  writeValueWithResponse?(value: BufferSource): Promise<void>;
  writeValueWithoutResponse?(value: BufferSource): Promise<void>;
};

type BrowserBluetoothCharacteristicEvent = Event & {
  target: BrowserBluetoothRemoteGattCharacteristic | null;
};

type BrowserUsb = {
  requestDevice(options: BrowserUsbRequestOptions): Promise<BrowserUsbDevice>;
};

type BrowserUsbRequestOptions = {
  filters: readonly BrowserUsbDeviceFilter[];
};

type BrowserUsbDeviceFilter = {
  vendorId?: number;
  productId?: number;
  classCode?: number;
  subclassCode?: number;
  protocolCode?: number;
  serialNumber?: string;
};

type BrowserUsbDevice = {
  readonly vendorId: number;
  readonly productId: number;
  readonly manufacturerName?: string;
  readonly productName?: string;
  readonly serialNumber?: string;
  readonly configurations: readonly BrowserUsbConfiguration[];
  readonly configuration?: BrowserUsbConfiguration | null;
  open(): Promise<void>;
  close(): Promise<void>;
  selectConfiguration(configurationValue: number): Promise<void>;
  claimInterface(interfaceNumber: number): Promise<void>;
  releaseInterface(interfaceNumber: number): Promise<void>;
  selectAlternateInterface?(
    interfaceNumber: number,
    alternateSetting: number,
  ): Promise<void>;
  transferIn(endpointNumber: number, length: number): Promise<BrowserUsbInTransferResult>;
  transferOut(
    endpointNumber: number,
    data: BufferSource,
  ): Promise<BrowserUsbOutTransferResult>;
};

type BrowserUsbConfiguration = {
  readonly configurationValue: number;
  readonly interfaces: readonly BrowserUsbInterface[];
};

type BrowserUsbInterface = {
  readonly interfaceNumber: number;
  readonly alternates: readonly BrowserUsbAlternateInterface[];
  readonly claimed?: boolean;
};

type BrowserUsbAlternateInterface = {
  readonly alternateSetting: number;
  readonly interfaceClass?: number;
  readonly interfaceSubclass?: number;
  readonly interfaceProtocol?: number;
  readonly endpoints: readonly BrowserUsbEndpoint[];
};

type BrowserUsbEndpoint = {
  readonly endpointNumber: number;
  readonly direction: "in" | "out";
  readonly type: "bulk" | "interrupt" | "isochronous";
  readonly packetSize: number;
};

type BrowserUsbInTransferResult = {
  readonly data?: DataView;
  readonly status: "ok" | "stall" | "babble";
};

type BrowserUsbOutTransferResult = {
  readonly bytesWritten: number;
  readonly status: "ok" | "stall";
};

type BrowserSerial = {
  requestPort(): Promise<BrowserSerialPort>;
};

type BrowserSerialPort = {
  readonly readable?: ReadableStream<Uint8Array> | null;
  readonly writable?: WritableStream<Uint8Array> | null;
  open(options: BrowserSerialOpenOptions): Promise<void>;
  close(): Promise<void>;
  getInfo?(): BrowserSerialPortInfo;
};

type BrowserSerialOpenOptions = {
  baudRate: number;
  bufferSize?: number;
};

type BrowserSerialPortInfo = {
  usbVendorId?: number;
  usbProductId?: number;
};

type UsbAutoInboundMessage =
  | { type: "hello" }
  | { type: "helloAck"; tag: Uint8Array }
  | { type: "data"; bytes: Uint8Array };

type BluetoothControl =
  | { type: "hello"; identity: Uint8Array }
  | { type: "welcome"; identity: Uint8Array }
  | { type: "close"; reason: string };

const USB_AUTO_WEB_SERIAL_BAUD_RATE = 115_200;
const USB_AUTO_PROBE_INTERVAL_MS = 500;
const USB_AUTO_OUTBOUND_POLL_MS = 25;
const WEBUSB_MIN_TRANSFER_BYTES = 512;
const BLUETOOTH_HANDSHAKE_TIMEOUT_MS = 10_000;
const BLUETOOTH_OUTBOUND_POLL_MS = 25;
const WEBSOCKET_OUTBOUND_POLL_MS = 25;
let nextBrowserUsbAutoTag = 0;
let nextBrowserWebSocketTag = 0;
const LINUX_WEBUSB_SETUP_HINT =
  "On Linux, run ./scripts/install-prns-webusb-udev.sh from the Prns repo root, " +
  "then unplug/replug the device and restart the browser. If this is Snap Chromium, " +
  "also run sudo snap connect chromium:raw-usb or use a non-Snap Chrome/Chromium build.";

export class BrowserLocalStorageIdentityStore implements IdentityStore {
  #key: string;

  constructor(key: string = "prns.identity.v1") {
    this.#key = key;
  }

  async load(expectedLength: number): Promise<IdentitySecretKey | null> {
    const storage = requireLocalStorage();
    const encoded = storage.getItem(this.#key);
    return encoded ? identitySecretKey(decodeBase64(encoded), expectedLength) : null;
  }

  async save(secretKey: IdentitySecretKey): Promise<void> {
    requireLocalStorage().setItem(this.#key, encodeBase64(secretKey));
  }
}

export type EntropySource = (length: number) => Uint8Array;

export type PrnsOptions = {
  wasm: PrnsWasmModule;
  identityStore?: IdentityStore;
  entropy?: EntropySource;
  now?: () => InstantMillis;
};

export type InterfaceConnectState =
  | "idle"
  | "requesting"
  | "opening"
  | "port-open"
  | "handshaking"
  | "peer-confirmed"
  | "failed"
  | "closed";

export type InterfaceFailure = {
  readonly code: PrnsValidationCode;
  readonly message: string;
};

export type InterfaceSession = {
  readonly name: InterfaceName;
  readonly interfaceId: InterfaceId;
  readonly state: InterfaceConnectState;
  readonly failure: InterfaceFailure | undefined;
  close(): Promise<void>;
};

export type UsbAutoSession = InterfaceSession & {
  readonly name: "usb-auto";
  readonly peerConfirmed: boolean;
};

export type BluetoothSession = InterfaceSession & {
  readonly name: "bluetooth";
  readonly peerConfirmed: boolean;
};

export type WebSocketSession = InterfaceSession & {
  readonly name: "websocket";
  readonly url: string;
  readonly connected: boolean;
};

export type UsbAutoDeviceFilter = {
  readonly vendorId?: number;
  readonly productId?: number;
  readonly serialNumber?: string;
};

export type UsbAutoConnectOptions = {
  readonly filters?: readonly UsbAutoDeviceFilter[];
};

export type WebSocketConnectOptions = {
  readonly protocols?: string | readonly string[];
  readonly channelTag?: ChannelTag;
  readonly bitrateBps?: BitrateBps;
  readonly hardwareMtu?: HardwareMtu;
};

export class PrnsInterfaces {
  readonly usbAuto: UsbAutoInterface;
  readonly rnode: RNodeInterface;
  readonly bluetooth: BluetoothInterface;
  readonly webSocket: WebSocketInterface;

  constructor(host: RuntimeHost) {
    this.usbAuto = new UsbAutoInterface(host);
    this.rnode = new RNodeInterface(host);
    this.bluetooth = new BluetoothInterface(host);
    this.webSocket = new WebSocketInterface(host);
  }
}

export class UsbAutoInterface {
  readonly name = "usb-auto" as const;
  readonly #host: RuntimeHost;

  constructor(host: RuntimeHost) {
    this.#host = host;
  }

  async connect(options: UsbAutoConnectOptions = {}): Promise<UsbAutoSession> {
    this.#host.assertReady();
    const usb = requireWebUsb();
    const device = await usbStage("request browser USB device", () =>
      usb.requestDevice({
        filters: options.filters ?? this.#host.defaultUsbAutoFilters(),
      }),
    );
    const transport = await WebUsbAutoTransport.open(device);

    let session: BrowserUsbAutoSession | undefined;
    try {
      const interfaceId = this.#host.registerInterface({
        kind: "auto-usb-host",
        channelTag: browserUsbAutoChannelTag(device),
        bitrateBps: this.#host.usbAutoHostBitrateBps(),
        hardwareMtu: this.#host.usbAutoHostHardwareMtu(),
      });
      session = new BrowserUsbAutoSession(this.#host, transport, interfaceId);
      session.start();
      return session;
    } catch (error) {
      await session?.close();
      if (!session) {
        await transport.close();
      }
      throw error;
    }
  }
}

class BrowserUsbAutoSession implements UsbAutoSession {
  readonly name = "usb-auto" as const;
  readonly interfaceId: InterfaceId;

  readonly #host: RuntimeHost;
  readonly #transport: WebUsbAutoTransport;
  readonly #decoder: UsbAutoDecoderBinding;
  readonly #nodeTag: Uint8Array;
  #writeQueue: Promise<void> = Promise.resolve();
  #closed = false;
  #confirmed = false;
  #state: InterfaceConnectState = "port-open";
  #failure: InterfaceFailure | undefined;

  constructor(
    host: RuntimeHost,
    transport: WebUsbAutoTransport,
    interfaceId: InterfaceId,
  ) {
    this.#host = host;
    this.#transport = transport;
    this.interfaceId = interfaceId;
    this.#decoder = host.createUsbAutoDecoder();
    this.#nodeTag = host.usbAutoNodeTagFor(interfaceId);
  }

  get state(): InterfaceConnectState {
    return this.#state;
  }

  get failure(): InterfaceFailure | undefined {
    return this.#failure;
  }

  get peerConfirmed(): boolean {
    return this.#confirmed;
  }

  start(): void {
    void this.#readLoop();
    void this.#probeLoop();
    void this.#outboundLoop();
  }

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    if (this.#state !== "failed") {
      this.#state = "closed";
    }
    await this.#writeQueue.catch(ignoreError);
    await this.#transport.close();
  }

  async #readLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        const chunk = await this.#transport.read();
        if (!chunk) {
          break;
        }
        if (chunk.length === 0) {
          continue;
        }
        for (const raw of this.#decoder.feed(chunk)) {
          await this.#handleInbound(parseUsbAutoMessage(raw));
        }
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(error);
      }
    } finally {
      if (!this.#closed) {
        await this.close();
      }
    }
  }

  async #probeLoop(): Promise<void> {
    try {
      while (!this.#closed && !this.#confirmed) {
        this.#state = "handshaking";
        await this.#writeFrame(this.#host.usbAutoHostHelloFrame());
        await delay(USB_AUTO_PROBE_INTERVAL_MS);
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(error);
      }
    }
  }

  async #outboundLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        if (this.#confirmed) {
          const frames = this.#host.takeOutboundFor(
            this.interfaceId,
            "auto-usb-host",
          );
          for (const frame of frames) {
            await this.#writeFrame(this.#host.usbAutoDataFrame(frame.bytes));
          }
        }
        await delay(USB_AUTO_OUTBOUND_POLL_MS);
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(error);
      }
    }
  }

  async #handleInbound(message: UsbAutoInboundMessage): Promise<void> {
    switch (message.type) {
      case "hello":
        await this.#writeFrame(this.#host.usbAutoHostHelloAckFrame(this.#nodeTag));
        this.#confirmPeer();
        break;
      case "helloAck":
        this.#confirmPeer();
        break;
      case "data":
        if (this.#confirmed && message.bytes.length > 0) {
          this.#host.ingest(this.interfaceId, packetFrame(message.bytes));
        }
        break;
    }
  }

  #confirmPeer(): void {
    this.#confirmed = true;
    this.#state = "peer-confirmed";
  }

  async #fail(error: unknown): Promise<void> {
    this.#failure = interfaceFailure(error);
    this.#state = "failed";
    this.#closed = true;
    await this.#writeQueue.catch(ignoreError);
    await this.#transport.close();
  }

  async #writeFrame(frame: Uint8Array): Promise<void> {
    if (this.#closed) {
      return;
    }
    const write = this.#writeQueue.then(async () => {
      if (!this.#closed) {
        await this.#transport.write(frame);
      }
    });
    this.#writeQueue = write.catch(ignoreError);
    await write;
  }
}

class WebUsbAutoTransport {
  readonly #device: BrowserUsbDevice;
  readonly #interfaceNumber: number;
  readonly #inEndpoint: BrowserUsbEndpoint;
  readonly #outEndpoint: BrowserUsbEndpoint;
  #closed = false;

  private constructor(
    device: BrowserUsbDevice,
    interfaceNumber: number,
    inEndpoint: BrowserUsbEndpoint,
    outEndpoint: BrowserUsbEndpoint,
  ) {
    this.#device = device;
    this.#interfaceNumber = interfaceNumber;
    this.#inEndpoint = inEndpoint;
    this.#outEndpoint = outEndpoint;
  }

  static async open(device: BrowserUsbDevice): Promise<WebUsbAutoTransport> {
    await usbStage("open selected USB device", () => device.open());
    const configuration = device.configuration ?? firstUsbConfiguration(device);
    if (!device.configuration) {
      await usbStage(`select USB configuration ${configuration.configurationValue}`, () =>
        device.selectConfiguration(configuration.configurationValue),
      );
    }
    const selectedConfiguration = device.configuration ?? configuration;
    const endpoints = findWebUsbEndpointPair(selectedConfiguration);
    if (!endpoints) {
      await device.close().catch(ignoreError);
      throw new PrnsValidationError(
        "invalid-component",
        "Selected USB device has no usable IN/OUT endpoint pair",
      );
    }
    await usbStage(`claim USB interface ${endpoints.interfaceNumber}`, () =>
      device.claimInterface(endpoints.interfaceNumber),
    );
    if (
      endpoints.alternate.alternateSetting !== 0 &&
      device.selectAlternateInterface
    ) {
      await usbStage(
        `select USB alternate ${endpoints.alternate.alternateSetting} on interface ${endpoints.interfaceNumber}`,
        () =>
          device.selectAlternateInterface!(
            endpoints.interfaceNumber,
            endpoints.alternate.alternateSetting,
          ),
      );
    }
    return new WebUsbAutoTransport(
      device,
      endpoints.interfaceNumber,
      endpoints.inEndpoint,
      endpoints.outEndpoint,
    );
  }

  async read(): Promise<Uint8Array | undefined> {
    if (this.#closed) {
      return undefined;
    }
    const length = Math.max(this.#inEndpoint.packetSize, WEBUSB_MIN_TRANSFER_BYTES);
    const result = await this.#device.transferIn(this.#inEndpoint.endpointNumber, length);
    if (result.status !== "ok") {
      throw new PrnsValidationError(
        "transfer-failed",
        `USB IN transfer failed with status ${result.status}`,
      );
    }
    const data = result.data;
    if (!data) {
      return new Uint8Array();
    }
    return new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
  }

  async write(bytes: Uint8Array): Promise<void> {
    if (this.#closed || bytes.length === 0) {
      return;
    }
    const result = await this.#device.transferOut(
      this.#outEndpoint.endpointNumber,
      arrayBufferForUsb(bytes),
    );
    if (result.status !== "ok" || result.bytesWritten !== bytes.length) {
      throw new PrnsValidationError(
        "transfer-failed",
        `USB OUT transfer wrote ${result.bytesWritten}/${bytes.length} bytes with status ${result.status}`,
      );
    }
  }

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    await this.#device.releaseInterface(this.#interfaceNumber).catch(ignoreError);
    await this.#device.close().catch(ignoreError);
  }
}

export class WebSocketInterface {
  readonly name = "websocket" as const;
  readonly #host: RuntimeHost;

  constructor(host: RuntimeHost) {
    this.#host = host;
  }

  async connect(
    url: string | URL,
    options: WebSocketConnectOptions = {},
  ): Promise<WebSocketSession> {
    this.#host.assertReady();
    const target = url.toString();
    const socket = await openBrowserWebSocket(target, options.protocols);
    const interfaceId = this.#host.registerInterface({
      kind: "websocket-client",
      channelTag: options.channelTag ?? browserWebSocketChannelTag(target),
      bitrateBps: options.bitrateBps ?? this.#host.websocketBitrateBps(),
      hardwareMtu: options.hardwareMtu ?? this.#host.websocketHardwareMtu(),
    });
    const session = new BrowserWebSocketSession(this.#host, socket, interfaceId, target);
    session.start();
    return session;
  }
}

class BrowserWebSocketSession implements WebSocketSession {
  readonly name = "websocket" as const;
  readonly interfaceId: InterfaceId;
  readonly url: string;

  readonly #host: RuntimeHost;
  readonly #socket: WebSocket;
  #writeQueue: Promise<void> = Promise.resolve();
  #closed = false;
  #state: InterfaceConnectState = "peer-confirmed";
  #failure: InterfaceFailure | undefined;

  constructor(
    host: RuntimeHost,
    socket: WebSocket,
    interfaceId: InterfaceId,
    url: string,
  ) {
    this.#host = host;
    this.#socket = socket;
    this.interfaceId = interfaceId;
    this.url = url;
  }

  get state(): InterfaceConnectState {
    return this.#state;
  }

  get failure(): InterfaceFailure | undefined {
    return this.#failure;
  }

  get connected(): boolean {
    return !this.#closed && this.#socket.readyState === WebSocket.OPEN;
  }

  start(): void {
    this.#socket.addEventListener("message", (event) => {
      void this.#handleMessage(event);
    });
    this.#socket.addEventListener("close", () => {
      this.#handleClose();
    });
    this.#socket.addEventListener("error", () => {
      void this.#fail(
        new PrnsValidationError(
          "disconnected",
          `WebSocket connection failed for ${this.url}`,
        ),
      );
    });
    void this.#outboundLoop();
  }

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    if (this.#state !== "failed") {
      this.#state = "closed";
    }
    await this.#writeQueue.catch(ignoreError);
    if (
      this.#socket.readyState === WebSocket.CONNECTING ||
      this.#socket.readyState === WebSocket.OPEN
    ) {
      this.#socket.close();
    }
  }

  async #handleMessage(event: MessageEvent): Promise<void> {
    try {
      const bytes = await websocketMessageBytes(event.data);
      if (bytes.length > 0 && !this.#closed) {
        this.#host.ingest(this.interfaceId, packetFrame(bytes));
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(error);
      }
    }
  }

  #handleClose(): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    if (this.#state !== "failed") {
      this.#state = "closed";
    }
  }

  async #outboundLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        const frames = this.#host.takeOutboundFor(
          this.interfaceId,
          "websocket-client",
        );
        for (const frame of frames) {
          await this.#writeFrame(frame.bytes);
        }
        await delay(WEBSOCKET_OUTBOUND_POLL_MS);
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(error);
      }
    }
  }

  async #fail(error: unknown): Promise<void> {
    this.#failure = interfaceFailure(error);
    this.#state = "failed";
    this.#closed = true;
    await this.#writeQueue.catch(ignoreError);
    if (
      this.#socket.readyState === WebSocket.CONNECTING ||
      this.#socket.readyState === WebSocket.OPEN
    ) {
      this.#socket.close();
    }
  }

  async #writeFrame(frame: Uint8Array): Promise<void> {
    if (this.#closed || frame.length === 0) {
      return;
    }
    const write = this.#writeQueue.then(() => {
      if (this.#closed) {
        return;
      }
      if (this.#socket.readyState !== WebSocket.OPEN) {
        throw new PrnsValidationError(
          "disconnected",
          `WebSocket is not open for ${this.url}`,
        );
      }
      this.#socket.send(arrayBufferForWebSocket(frame));
    });
    this.#writeQueue = write.catch(ignoreError);
    await write;
  }
}

export class RNodeInterface {
  readonly name = "rnode" as const;
  readonly #host: RuntimeHost;

  constructor(host: RuntimeHost) {
    this.#host = host;
  }

  async connect(): Promise<InterfaceSession> {
    this.#host.assertReady();
    throw new PrnsValidationError(
      "missing-host-api",
      "RNode browser connection is not wired yet",
    );
  }
}

export class BluetoothInterface {
  readonly name = "bluetooth" as const;
  readonly #host: RuntimeHost;

  constructor(host: RuntimeHost) {
    this.#host = host;
  }

  async connect(): Promise<BluetoothSession> {
    this.#host.assertReady();
    const bluetooth = requireWebBluetooth();
    const serviceUuid = this.#host.bluetoothServiceUuid();
    const device = await bluetooth.requestDevice({
      filters: [{ services: [serviceUuid] }],
      optionalServices: [serviceUuid],
    });
    const server = await device.gatt?.connect();
    if (!server) {
      throw new PrnsValidationError(
        "missing-host-api",
        "Web Bluetooth device did not expose a GATT server",
      );
    }
    const service = await server.getPrimaryService(serviceUuid);
    const control = await service.getCharacteristic(this.#host.bluetoothControlUuid());
    const data = await optionalBluetoothCharacteristic(
      service,
      this.#host.bluetoothDataUuid(),
    );
    const session = new BrowserBluetoothSession(
      this.#host,
      server,
      control,
      data ?? control,
    );
    await session.start();
    return session;
  }
}

class BrowserBluetoothSession implements BluetoothSession {
  readonly name = "bluetooth" as const;
  readonly #host: RuntimeHost;
  readonly #server: BrowserBluetoothRemoteGattServer;
  readonly #control: BrowserBluetoothRemoteGattCharacteristic;
  readonly #data: BrowserBluetoothRemoteGattCharacteristic;
  readonly #reassembler: BluetoothReassemblerBinding;
  #interfaceId?: InterfaceId;
  #writeQueue: Promise<void> = Promise.resolve();
  #closed = false;
  #confirmed = false;
  #state: InterfaceConnectState = "opening";
  #failure: InterfaceFailure | undefined;

  constructor(
    host: RuntimeHost,
    server: BrowserBluetoothRemoteGattServer,
    control: BrowserBluetoothRemoteGattCharacteristic,
    data: BrowserBluetoothRemoteGattCharacteristic,
  ) {
    this.#host = host;
    this.#server = server;
    this.#control = control;
    this.#data = data;
    this.#reassembler = host.createBluetoothReassembler();
  }

  get interfaceId(): InterfaceId {
    if (!this.#interfaceId) {
      throw new PrnsValidationError(
        "invalid-component",
        "Bluetooth peer interface is not registered yet",
      );
    }
    return this.#interfaceId;
  }

  get state(): InterfaceConnectState {
    return this.#state;
  }

  get failure(): InterfaceFailure | undefined {
    return this.#failure;
  }

  get peerConfirmed(): boolean {
    return this.#confirmed;
  }

  async start(): Promise<void> {
    this.#state = "handshaking";
    await this.#control.startNotifications();
    this.#control.addEventListener("characteristicvaluechanged", (event) => {
      this.#handleControlEvent(event as BrowserBluetoothCharacteristicEvent);
    });
    if (this.#data !== this.#control) {
      await this.#data.startNotifications();
      this.#data.addEventListener("characteristicvaluechanged", (event) => {
        this.#handleDataEvent(event as BrowserBluetoothCharacteristicEvent);
      });
    }
    await this.#writeControl(this.#host.bluetoothDialerHello());
    await this.#waitForPeer();
    void this.#outboundLoop();
  }

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    if (this.#state !== "failed") {
      this.#state = "closed";
    }
    await this.#writeQueue.catch(ignoreError);
    this.#server.disconnect();
  }

  async #waitForPeer(): Promise<void> {
    const started = Date.now();
    while (!this.#confirmed && !this.#closed) {
      if (Date.now() - started > BLUETOOTH_HANDSHAKE_TIMEOUT_MS) {
        await this.close();
        throw new PrnsValidationError(
          "invalid-component",
          "Bluetooth handshake timed out before Welcome",
        );
      }
      await delay(25);
    }
  }

  #handleControlEvent(event: BrowserBluetoothCharacteristicEvent): void {
    const bytes = characteristicBytes(event);
    const control = parseBluetoothControl(this.#host.bluetoothDecodeControl(bytes));
    if (control.type === "welcome") {
      this.#interfaceId = this.#host.registerInterface({
        kind: "bluetooth-peer",
        channelTag: channelTag(control.identity),
        bitrateBps: this.#host.bluetoothBitrateBps(),
        hardwareMtu: this.#host.bluetoothHardwareMtu(),
      });
      this.#confirmed = true;
      this.#state = "peer-confirmed";
      return;
    }
    if (control.type === "close") {
      void this.close();
      return;
    }
    if (this.#data === this.#control) {
      this.#handleDataBytes(bytes);
    }
  }

  #handleDataEvent(event: BrowserBluetoothCharacteristicEvent): void {
    this.#handleDataBytes(characteristicBytes(event));
  }

  #handleDataBytes(bytes: Uint8Array): void {
    if (!this.#confirmed || !this.#interfaceId) {
      return;
    }
    const frame = this.#reassembler.absorb(bytes);
    if (frame && frame.length > 0) {
      this.#host.ingest(this.#interfaceId, packetFrame(frame));
    }
  }

  async #outboundLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        const interfaceId = this.#interfaceId;
        if (this.#confirmed && interfaceId) {
          const frames = this.#host.takeOutboundFor(interfaceId, "bluetooth-auto");
          for (const frame of frames) {
            for (const fragment of this.#host.bluetoothDataFragments(frame.bytes)) {
              await this.#writeData(fragment);
            }
          }
        }
        await delay(BLUETOOTH_OUTBOUND_POLL_MS);
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(error);
      }
    }
  }

  async #fail(error: unknown): Promise<void> {
    this.#failure = interfaceFailure(error);
    this.#state = "failed";
    this.#closed = true;
    await this.#writeQueue.catch(ignoreError);
    this.#server.disconnect();
  }

  async #writeControl(bytes: Uint8Array): Promise<void> {
    await this.#write(this.#control, bytes);
  }

  async #writeData(bytes: Uint8Array): Promise<void> {
    await this.#write(this.#data, bytes);
  }

  async #write(
    characteristic: BrowserBluetoothRemoteGattCharacteristic,
    bytes: Uint8Array,
  ): Promise<void> {
    const write = this.#writeQueue.then(async () => {
      if (!this.#closed) {
        await writeBluetoothValue(characteristic, bytes);
      }
    });
    this.#writeQueue = write.catch(ignoreError);
    await write;
  }
}

export class Prns {
  readonly interfaces: PrnsInterfaces;
  #runtime: PrnsRuntimeBinding;
  #entropy: EntropySource;
  #now: () => InstantMillis;

  private constructor(
    wasm: PrnsWasmModule,
    runtime: PrnsRuntimeBinding,
    entropy: EntropySource,
    now: () => InstantMillis,
  ) {
    this.#runtime = runtime;
    this.#entropy = entropy;
    this.#now = now;
    this.interfaces = new PrnsInterfaces(
      new RuntimeHost(wasm, runtime, entropy, now),
    );
  }

  static async create(options: PrnsOptions): Promise<Prns> {
    const identityLength = options.wasm.identitySecretKeyLength();
    const store = options.identityStore;
    let identity = store ? await store.load(identityLength) : null;
    if (!identity) {
      identity = identitySecretKey(webCryptoBytes(identityLength), identityLength);
      await store?.save(identity);
    }
    return new Prns(
      options.wasm,
      new options.wasm.PrnsRuntime(identity),
      options.entropy ?? webCryptoBytes,
      options.now ?? nowMillis,
    );
  }

  registerSingleDestination(
    options: RegisterSingleDestinationOptions,
  ): DestinationHash {
    return destinationHash(this.#runtime.registerSingleDestination(options));
  }

  announce(destination: DestinationHash): CommandId {
    return commandId(
      this.#runtime.announce({
        destination,
        nowMs: this.#now(),
        entropy: this.#entropyBytes(),
      }),
    );
  }

  drainEvents(): PrnsEvent[] {
    return this.#runtime.drainEvents().map(parseEvent);
  }

  snapshot(): PrnsSnapshot {
    return parseSnapshot(this.#runtime.snapshot());
  }

  #entropyBytes(): EntropyBytes {
    return entropyBytes(this.#entropy(MIN_ENTROPY_BYTES));
  }
}

class RuntimeHost {
  readonly #wasm: PrnsWasmModule;
  readonly #runtime: PrnsRuntimeBinding;
  readonly #entropy: EntropySource;
  readonly #now: () => InstantMillis;
  #pendingOutbound: PrnsOutboundFrame[] = [];

  constructor(
    wasm: PrnsWasmModule,
    runtime: PrnsRuntimeBinding,
    entropy: EntropySource,
    now: () => InstantMillis,
  ) {
    this.#wasm = wasm;
    this.#runtime = runtime;
    this.#entropy = entropy;
    this.#now = now;
  }

  assertReady(): void {
    this.#runtime.snapshot();
  }

  registerInterface(options: RuntimeRegisterInterfaceOptions): InterfaceId {
    return interfaceId(this.#runtime.registerInterface(options));
  }

  ingest(interfaceId: InterfaceId, bytes: PacketFrame): void {
    this.#runtime.ingest({
      interfaceId,
      bytes,
      nowMs: this.#now(),
      entropy: this.entropy(),
    });
  }

  drainOutbound(): PrnsOutboundFrame[] {
    return this.#runtime.drainOutbound().map(parseOutboundFrame);
  }

  takeOutboundFor(
    interfaceId: InterfaceId,
    supervisorKind: RuntimeInterfaceKind,
  ): PrnsOutboundFrame[] {
    this.#pendingOutbound.push(...this.drainOutbound());
    const picked: PrnsOutboundFrame[] = [];
    const pending: PrnsOutboundFrame[] = [];
    for (const frame of this.#pendingOutbound) {
      if (outboundTargets(frame.target, interfaceId, supervisorKind)) {
        picked.push(frame);
      } else {
        pending.push(frame);
      }
    }
    this.#pendingOutbound = pending;
    return picked;
  }

  createUsbAutoDecoder(): UsbAutoDecoderBinding {
    return new this.#wasm.UsbAutoDecoder();
  }

  createBluetoothReassembler(): BluetoothReassemblerBinding {
    return new this.#wasm.BluetoothReassembler();
  }

  bluetoothServiceUuid(): string {
    return this.#wasm.bluetoothServiceUuid();
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

  websocketHardwareMtu(): HardwareMtu {
    return hardwareMtu(this.#wasm.websocketHardwareMtu());
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

  entropy(): EntropyBytes {
    return entropyBytes(this.#entropy(MIN_ENTROPY_BYTES));
  }
}

export function identitySecretKey(
  bytes: Uint8Array,
  expectedLength: number,
): IdentitySecretKey {
  return exactBytes(bytes, expectedLength, "IdentitySecretKey") as IdentitySecretKey;
}

export function interfaceId(bytes: Uint8Array): InterfaceId {
  return exactBytes(bytes, INTERFACE_ID_LENGTH, "InterfaceId") as InterfaceId;
}

export function destinationHash(bytes: Uint8Array): DestinationHash {
  return exactBytes(
    bytes,
    DESTINATION_HASH_LENGTH,
    "DestinationHash",
  ) as DestinationHash;
}

export function channelTag(bytes: Uint8Array): ChannelTag {
  return nonEmptyBytes(bytes, "ChannelTag") as ChannelTag;
}

export function packetFrame(bytes: Uint8Array): PacketFrame {
  return nonEmptyBytes(bytes, "PacketFrame") as PacketFrame;
}

export function entropyBytes(bytes: Uint8Array): EntropyBytes {
  if (bytes.length < MIN_ENTROPY_BYTES) {
    throw new PrnsValidationError(
      "invalid-length",
      `EntropyBytes requires at least ${MIN_ENTROPY_BYTES} bytes`,
    );
  }
  return copyBytes(bytes) as EntropyBytes;
}

export function appData(bytes: Uint8Array = new Uint8Array()): AppData {
  return copyBytes(bytes) as AppData;
}

export function appName(value: string): AppName {
  return dottedComponent(value, "AppName") as AppName;
}

export function aspect(value: string): Aspect {
  return dottedComponent(value, "Aspect") as Aspect;
}

export function bitrateBps(value: number): BitrateBps {
  return positiveInteger(value, "BitrateBps") as BitrateBps;
}

export function hardwareMtu(value: number): HardwareMtu {
  return positiveInteger(value, "HardwareMtu") as HardwareMtu;
}

export function hopCount(value: number): HopCount {
  if (!Number.isInteger(value) || value < 0 || value > 255) {
    throw new PrnsValidationError(
      "invalid-number",
      "HopCount must be an integer from 0 through 255",
    );
  }
  return value as HopCount;
}

export function nowMillis(value: number = Date.now()): InstantMillis {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new PrnsValidationError(
      "invalid-number",
      "InstantMillis must be a non-negative safe integer",
    );
  }
  return value as InstantMillis;
}

export function commandId(value: bigint): CommandId {
  if (value < 0n) {
    throw new PrnsValidationError(
      "invalid-number",
      "CommandId must be non-negative",
    );
  }
  return value as CommandId;
}

export function webCryptoEntropy(length: number): EntropyBytes {
  return entropyBytes(webCryptoBytes(length));
}

function outboundTargets(
  target: OutboundTarget,
  interfaceId: InterfaceId,
  supervisorKind: RuntimeInterfaceKind,
): boolean {
  if (target.type === "interface") {
    return equalBytes(target.interfaceId, interfaceId);
  }
  if (target.supervisorKind !== supervisorKind) {
    return false;
  }
  switch (target.fan.type) {
    case "all":
      return true;
    case "only":
      return equalBytes(target.fan.interfaceId, interfaceId);
    case "allExcept":
      return !equalBytes(target.fan.interfaceId, interfaceId);
  }
}

function parseUsbAutoMessage(raw: unknown): UsbAutoInboundMessage {
  const object = record(raw, "UsbAutoInboundMessage");
  const type = stringField(object, "type");
  switch (type) {
    case "hello":
      return { type };
    case "helloAck":
      return { type, tag: bytesField(object, "tag") };
    case "data":
      return { type, bytes: bytesField(object, "bytes") };
    default:
      throw new PrnsValidationError(
        "invalid-component",
        `unknown USB-auto message ${type}`,
      );
  }
}

function parseBluetoothControl(raw: unknown): BluetoothControl {
  const object = record(raw, "BluetoothControl");
  const type = stringField(object, "type");
  switch (type) {
    case "hello":
    case "welcome":
      return { type, identity: bytesField(object, "identity") };
    case "close":
      return { type, reason: stringField(object, "reason") };
    default:
      throw new PrnsValidationError(
        "invalid-component",
        `unknown Bluetooth control ${type}`,
      );
  }
}

function parseOutboundFrame(raw: unknown): PrnsOutboundFrame {
  const object = record(raw, "PrnsOutboundFrame");
  const type = stringField(object, "type");
  if (type !== "frame" && type !== "announce") {
    throw new PrnsValidationError(
      "unknown-outbound-target",
      `unknown outbound frame type ${type}`,
    );
  }
  const frame: PrnsOutboundFrame = {
    type,
    target: parseOutboundTarget(field(object, "target")),
    bytes: packetFrame(bytesField(object, "bytes")),
  };
  const hops = optionalNumber(object, "hops", hopCount);
  if (hops !== undefined) {
    frame.hops = hops;
  }
  return frame;
}

function parseOutboundTarget(raw: unknown): OutboundTarget {
  const object = record(raw, "OutboundTarget");
  const type = stringField(object, "type");
  if (type === "interface") {
    return {
      type,
      interfaceId: interfaceId(bytesField(object, "interfaceId")),
    };
  }
  if (type === "broadcast") {
    return {
      type,
      supervisorKind: parseRuntimeInterfaceKind(stringField(object, "supervisorKind")),
      fan: parseFanTarget(field(object, "fan")),
    };
  }
  throw new PrnsValidationError(
    "unknown-outbound-target",
    `unknown outbound target ${type}`,
  );
}

function parseFanTarget(raw: unknown): FanTarget {
  const object = record(raw, "FanTarget");
  const type = stringField(object, "type");
  if (type === "all") {
    return { type };
  }
  if (type === "only" || type === "allExcept") {
    return {
      type,
      interfaceId: interfaceId(bytesField(object, "interfaceId")),
    };
  }
  throw new PrnsValidationError(
    "unknown-outbound-target",
    `unknown fan target ${type}`,
  );
}

function parseEvent(raw: unknown): PrnsEvent {
  const object = record(raw, "PrnsEvent");
  const type = stringField(object, "type");
  switch (type) {
    case "announce":
      return {
        type,
        destination: destinationHash(bytesField(object, "destination")),
        hops: hopCount(numberField(object, "hops")),
        sourceInterface: interfaceId(bytesField(object, "sourceInterface")),
      };
    case "commandSettled":
      return {
        type,
        commandId: commandId(bigintField(object, "id")),
        debugSettlement: stringField(object, "settlement"),
      };
    case "routeExpired":
    case "routeEvicted":
    case "routeInterfaceGone":
    case "routeDropped":
      return {
        type,
        destination: destinationHash(bytesField(object, "destination")),
      };
    default:
      return { type: "unknown", raw };
  }
}

function parseSnapshot(raw: unknown): PrnsSnapshot {
  const object = record(raw, "PrnsSnapshot");
  const interfacesRaw = field(object, "interfaces");
  if (!Array.isArray(interfacesRaw)) {
    throw new PrnsValidationError(
      "invalid-component",
      "snapshot interfaces must be an array",
    );
  }
  return {
    type: literalField(object, "type", "snapshot"),
    ingestedPackets: nonNegativeInteger(
      numberField(object, "ingestedPackets"),
      "ingestedPackets",
    ),
    ingestedCommands: nonNegativeInteger(
      numberField(object, "ingestedCommands"),
      "ingestedCommands",
    ),
    routes: nonNegativeInteger(numberField(object, "routes"), "routes"),
    scheduledAnnounces: nonNegativeInteger(
      numberField(object, "scheduledAnnounces"),
      "scheduledAnnounces",
    ),
    interfaces: interfacesRaw.map(parseInterfaceSnapshot),
  };
}

function parseInterfaceSnapshot(raw: unknown): InterfaceSnapshot {
  const object = record(raw, "InterfaceSnapshot");
  const snapshot: InterfaceSnapshot = {
    id: interfaceId(bytesField(object, "id")),
    kind: stringField(object, "kind"),
    routes: nonNegativeInteger(numberField(object, "routes"), "routes"),
    links: nonNegativeInteger(numberField(object, "links"), "links"),
  };
  const bitrate = optionalNumber(object, "bitrateBps", bitrateBps);
  if (bitrate !== undefined) {
    snapshot.bitrateBps = bitrate;
  }
  const mtu = optionalNumber(object, "hardwareMtu", hardwareMtu);
  if (mtu !== undefined) {
    snapshot.hardwareMtu = mtu;
  }
  return snapshot;
}

function parseRuntimeInterfaceKind(value: string): RuntimeInterfaceKind {
  if (
    value === "auto-usb-host" ||
    value === "auto-usb-device" ||
    value === "rnode" ||
    value === "bluetooth-auto" ||
    value === "bluetooth-peer" ||
    value === "websocket-client" ||
    value === "websocket-server" ||
    value === "websocket-server-peer" ||
    value === "serial" ||
    value === "kiss" ||
    value === "pipe"
  ) {
    return value;
  }
  throw new PrnsValidationError(
    "unknown-interface-kind",
    `unknown interface kind ${value}`,
  );
}

function exactBytes(
  bytes: Uint8Array,
  expectedLength: number,
  name: string,
): Uint8Array {
  if (bytes.length !== expectedLength) {
    throw new PrnsValidationError(
      "invalid-length",
      `${name} must be ${expectedLength} bytes`,
    );
  }
  return copyBytes(bytes);
}

function nonEmptyBytes(bytes: Uint8Array, name: string): Uint8Array {
  if (bytes.length === 0) {
    throw new PrnsValidationError("empty-bytes", `${name} must not be empty`);
  }
  return copyBytes(bytes);
}

function copyBytes(bytes: Uint8Array): Uint8Array {
  return new Uint8Array(bytes);
}

function dottedComponent(value: string, name: string): string {
  if (value.length === 0) {
    throw new PrnsValidationError("empty-string", `${name} must not be empty`);
  }
  if (value.includes(".")) {
    throw new PrnsValidationError(
      "invalid-component",
      `${name} must not contain dots`,
    );
  }
  return value;
}

function positiveInteger(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new PrnsValidationError(
      "invalid-number",
      `${name} must be a positive safe integer`,
    );
  }
  return value;
}

function nonNegativeInteger(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new PrnsValidationError(
      "invalid-number",
      `${name} must be a non-negative safe integer`,
    );
  }
  return value;
}

function field(object: Record<string, unknown>, key: string): unknown {
  if (!(key in object)) {
    throw new PrnsValidationError(
      "invalid-component",
      `missing field ${key}`,
    );
  }
  return object[key];
}

function stringField(object: Record<string, unknown>, key: string): string {
  const value = field(object, key);
  if (typeof value !== "string") {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be a string`,
    );
  }
  return value;
}

function literalField<T extends string>(
  object: Record<string, unknown>,
  key: string,
  expected: T,
): T {
  const value = stringField(object, key);
  if (value !== expected) {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be ${expected}`,
    );
  }
  return expected;
}

function numberField(object: Record<string, unknown>, key: string): number {
  const value = field(object, key);
  if (typeof value !== "number") {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be a number`,
    );
  }
  return value;
}

function optionalNumber<T>(
  object: Record<string, unknown>,
  key: string,
  parse: (value: number) => T,
): T | undefined {
  if (!(key in object)) {
    return undefined;
  }
  return parse(numberField(object, key));
}

function bigintField(object: Record<string, unknown>, key: string): bigint {
  const value = field(object, key);
  if (typeof value === "bigint") {
    return value;
  }
  if (typeof value === "number" && Number.isSafeInteger(value)) {
    return BigInt(value);
  }
  throw new PrnsValidationError(
    "invalid-component",
    `${key} must be a bigint or safe integer`,
  );
}

function bytesField(object: Record<string, unknown>, key: string): Uint8Array {
  const value = field(object, key);
  if (!(value instanceof Uint8Array)) {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be a Uint8Array`,
    );
  }
  return value;
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new PrnsValidationError(
      "invalid-component",
      `${name} must be an object`,
    );
  }
  return value as Record<string, unknown>;
}

function webCryptoBytes(length: number): Uint8Array {
  if (!Number.isSafeInteger(length) || length <= 0) {
    throw new PrnsValidationError(
      "invalid-number",
      "random byte length must be a positive safe integer",
    );
  }
  const out = new Uint8Array(length);
  const crypto = hostGlobal().crypto;
  if (!crypto) {
    throw new PrnsValidationError(
      "missing-host-api",
      "Prns entropy requires globalThis.crypto.getRandomValues",
    );
  }
  crypto.getRandomValues(out);
  return out;
}

function encodeBase64(bytes: Uint8Array): string {
  const btoa = hostGlobal().btoa;
  if (!btoa) {
    throw new PrnsValidationError(
      "missing-host-api",
      "BrowserLocalStorageIdentityStore requires globalThis.btoa",
    );
  }
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

function decodeBase64(encoded: string): Uint8Array {
  const atob = hostGlobal().atob;
  if (!atob) {
    throw new PrnsValidationError(
      "missing-host-api",
      "BrowserLocalStorageIdentityStore requires globalThis.atob",
    );
  }
  const binary = atob(encoded);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}

function requireWebSerial(): BrowserSerial {
  const serial = hostGlobal().navigator?.serial;
  if (!serial) {
    throw new PrnsValidationError(
      "missing-host-api",
      "USB Auto requires the browser Web Serial API",
    );
  }
  return serial;
}

function requireWebUsb(): BrowserUsb {
  const usb = hostGlobal().navigator?.usb;
  if (!usb) {
    throw new PrnsValidationError(
      "missing-host-api",
      "USB Auto requires the browser WebUSB API",
    );
  }
  return usb;
}

function requireWebBluetooth(): BrowserBluetooth {
  const bluetooth = hostGlobal().navigator?.bluetooth;
  if (!bluetooth) {
    throw new PrnsValidationError(
      "missing-host-api",
      "Bluetooth requires the browser Web Bluetooth API",
    );
  }
  return bluetooth;
}

function requireBrowserWebSocket(): typeof WebSocket {
  const WebSocketCtor = hostGlobal().WebSocket;
  if (!WebSocketCtor) {
    throw new PrnsValidationError(
      "missing-host-api",
      "WebSocket interface requires globalThis.WebSocket",
    );
  }
  return WebSocketCtor;
}

function openBrowserWebSocket(
  url: string,
  protocols?: string | readonly string[],
): Promise<WebSocket> {
  const WebSocketCtor = requireBrowserWebSocket();
  const protocolList =
    protocols === undefined || typeof protocols === "string"
      ? protocols
      : [...protocols];
  const socket =
    protocolList === undefined
      ? new WebSocketCtor(url)
      : new WebSocketCtor(url, protocolList);
  socket.binaryType = "arraybuffer";
  return new Promise((resolve, reject) => {
    const cleanup = (): void => {
      socket.removeEventListener("open", handleOpen);
      socket.removeEventListener("error", handleError);
    };
    const handleOpen = (): void => {
      cleanup();
      resolve(socket);
    };
    const handleError = (): void => {
      cleanup();
      reject(
        new PrnsValidationError(
          "disconnected",
          `WebSocket connection failed for ${url}`,
        ),
      );
    };
    socket.addEventListener("open", handleOpen);
    socket.addEventListener("error", handleError);
  });
}

async function websocketMessageBytes(data: MessageEvent["data"]): Promise<Uint8Array> {
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data);
  }
  if (ArrayBuffer.isView(data)) {
    return new Uint8Array(
      data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength),
    );
  }
  if (typeof Blob !== "undefined" && data instanceof Blob) {
    return new Uint8Array(await data.arrayBuffer());
  }
  throw new PrnsValidationError(
    "unsupported-frame",
    "Prns WebSocket interfaces only accept binary messages",
  );
}

function firstUsbConfiguration(device: BrowserUsbDevice): BrowserUsbConfiguration {
  const configuration = device.configurations[0];
  if (!configuration) {
    throw new PrnsValidationError(
      "invalid-component",
      "Selected USB device has no configurations",
    );
  }
  return configuration;
}

type WebUsbEndpointPair = {
  interfaceNumber: number;
  alternate: BrowserUsbAlternateInterface;
  inEndpoint: BrowserUsbEndpoint;
  outEndpoint: BrowserUsbEndpoint;
};

function findWebUsbEndpointPair(
  configuration: BrowserUsbConfiguration,
): WebUsbEndpointPair | undefined {
  const vendorPairs: WebUsbEndpointPair[] = [];
  const bulkPairs: WebUsbEndpointPair[] = [];
  let fallbackPair: WebUsbEndpointPair | undefined;
  for (const iface of configuration.interfaces) {
    for (const alternate of iface.alternates) {
      const inEndpoint = alternate.endpoints.find(
        (endpoint) => endpoint.direction === "in" && endpoint.type === "bulk",
      );
      const outEndpoint = alternate.endpoints.find(
        (endpoint) => endpoint.direction === "out" && endpoint.type === "bulk",
      );
      if (inEndpoint && outEndpoint) {
        const pair = {
          interfaceNumber: iface.interfaceNumber,
          alternate,
          inEndpoint,
          outEndpoint,
        };
        if (alternate.interfaceClass === 0xff) {
          vendorPairs.push(pair);
        } else {
          bulkPairs.push(pair);
        }
        continue;
      }

      const fallbackIn = alternate.endpoints.find(
        (endpoint) => endpoint.direction === "in",
      );
      const fallbackOut = alternate.endpoints.find(
        (endpoint) => endpoint.direction === "out",
      );
      if (!fallbackPair && fallbackIn && fallbackOut) {
        fallbackPair = {
          interfaceNumber: iface.interfaceNumber,
          alternate,
          inEndpoint: fallbackIn,
          outEndpoint: fallbackOut,
        };
      }
    }
  }
  return vendorPairs[0] ?? bulkPairs[0] ?? fallbackPair;
}

async function usbStage<T>(stage: string, action: () => Promise<T>): Promise<T> {
  try {
    return await action();
  } catch (error) {
    const code = usbErrorCode(error);
    throw new PrnsValidationError(
      code,
      `USB ${stage} failed: ${describeUsbError(error, stage)}`,
    );
  }
}

function usbErrorCode(error: unknown): PrnsValidationCode {
  if (error instanceof DOMException) {
    if (error.name === "SecurityError") {
      return "permission-denied";
    }
    if (error.name === "NotFoundError") {
      return "operation-cancelled";
    }
    if (
      error.name === "NetworkError" ||
      error.name === "NotReadableError" ||
      error.name === "AbortError"
    ) {
      return "disconnected";
    }
  }
  return "invalid-component";
}

function describeUsbError(error: unknown, stage: string): string {
  const base = describeHostError(error);
  if (error instanceof DOMException && error.name === "SecurityError") {
    return `${base}. ${LINUX_WEBUSB_SETUP_HINT}`;
  }
  if (
    error instanceof DOMException &&
    error.name === "NotFoundError" &&
    stage.includes("request")
  ) {
    return `${base}. No USB device was selected.`;
  }
  return base;
}

function interfaceFailure(error: unknown): InterfaceFailure {
  if (error instanceof PrnsValidationError) {
    return { code: error.code, message: error.message };
  }
  if (error instanceof DOMException) {
    return {
      code: usbErrorCode(error),
      message: describeHostError(error),
    };
  }
  if (error instanceof Error) {
    return { code: "invalid-component", message: error.message };
  }
  return { code: "invalid-component", message: String(error) };
}

function describeHostError(error: unknown): string {
  if (error instanceof DOMException) {
    return `${error.name}: ${error.message}`;
  }
  if (error instanceof Error) {
    return `${error.name}: ${error.message}`;
  }
  return String(error);
}

async function optionalBluetoothCharacteristic(
  service: BrowserBluetoothRemoteGattService,
  uuid: string,
): Promise<BrowserBluetoothRemoteGattCharacteristic | undefined> {
  try {
    return await service.getCharacteristic(uuid);
  } catch {
    return undefined;
  }
}

function characteristicBytes(event: BrowserBluetoothCharacteristicEvent): Uint8Array {
  const value = event.target?.value;
  if (!value) {
    throw new PrnsValidationError(
      "invalid-component",
      "Bluetooth characteristic event did not include a value",
    );
  }
  return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
}

async function writeBluetoothValue(
  characteristic: BrowserBluetoothRemoteGattCharacteristic,
  bytes: Uint8Array,
): Promise<void> {
  const value = arrayBufferForBluetooth(bytes);
  if (characteristic.writeValueWithoutResponse) {
    await characteristic.writeValueWithoutResponse(value);
  } else if (characteristic.writeValueWithResponse) {
    await characteristic.writeValueWithResponse(value);
  } else if (characteristic.writeValue) {
    await characteristic.writeValue(value);
  } else {
    throw new PrnsValidationError(
      "missing-host-api",
      "Bluetooth characteristic does not support writes",
    );
  }
}

function arrayBufferForBluetooth(bytes: Uint8Array): ArrayBuffer {
  const out = new ArrayBuffer(bytes.length);
  new Uint8Array(out).set(bytes);
  return out;
}

function arrayBufferForUsb(bytes: Uint8Array): ArrayBuffer {
  const out = new ArrayBuffer(bytes.length);
  new Uint8Array(out).set(bytes);
  return out;
}

function arrayBufferForWebSocket(bytes: Uint8Array): ArrayBuffer {
  const out = new ArrayBuffer(bytes.length);
  new Uint8Array(out).set(bytes);
  return out;
}

function browserUsbAutoChannelTag(device: BrowserUsbDevice): ChannelTag {
  const vendor = formatOptionalHex(device.vendorId);
  const product = formatOptionalHex(device.productId);
  const serial = device.serialNumber ?? "unknown";
  const nonce = nextBrowserUsbAutoTag;
  nextBrowserUsbAutoTag = (nextBrowserUsbAutoTag + 1) >>> 0;
  return channelTag(
    new TextEncoder().encode(`webusb:auto-usb:${vendor}:${product}:${serial}:${nonce}`),
  );
}

function browserWebSocketChannelTag(url: string): ChannelTag {
  const nonce = nextBrowserWebSocketTag;
  nextBrowserWebSocketTag = (nextBrowserWebSocketTag + 1) >>> 0;
  return channelTag(new TextEncoder().encode(`websocket-client:${url}:${nonce}`));
}

function formatOptionalHex(value: number | undefined): string {
  return value === undefined ? "unknown" : value.toString(16).padStart(4, "0");
}

async function closeSerialPortQuietly(port: BrowserSerialPort): Promise<void> {
  await port.close().catch(ignoreError);
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function ignoreError(_error: unknown): void {}

function requireLocalStorage(): NonNullable<HostGlobal["localStorage"]> {
  const storage = hostGlobal().localStorage;
  if (!storage) {
    throw new PrnsValidationError(
      "missing-host-api",
      "BrowserLocalStorageIdentityStore requires globalThis.localStorage",
    );
  }
  return storage;
}

function hostGlobal(): HostGlobal {
  return globalThis as HostGlobal;
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
