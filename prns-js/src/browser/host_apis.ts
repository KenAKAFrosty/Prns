import type { Tag } from "../casework.js";

export type HostApi =
  | "Crypto"
  | "LocalStorage"
  | "Base64Encoder"
  | "Base64Decoder"
  | "WebUSB"
  | "WebBluetooth"
  | "WebSocket"
  | "Fetch";

export type HostApiUnavailable<Api extends HostApi = HostApi> = Tag<
  "HostApiUnavailable",
  { readonly api: Api }
>;

export type HostGlobal = typeof globalThis & {
  crypto?: {
    getRandomValues<T extends Uint8Array>(array: T): T;
  };
  navigator?: {
    bluetooth?: BrowserBluetooth;
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

export type BrowserBluetooth = {
  requestDevice(options: BrowserBluetoothRequestOptions): Promise<BrowserBluetoothDevice>;
};

export type BrowserBluetoothRequestOptions = {
  filters: readonly BrowserBluetoothRequestFilter[];
  optionalServices?: readonly string[];
};

export type BrowserBluetoothRequestFilter = {
  services: readonly string[];
};

export type BrowserBluetoothDevice = EventTarget & {
  readonly gatt?: BrowserBluetoothRemoteGattServer;
};

export type BrowserBluetoothRemoteGattServer = {
  connect(): Promise<BrowserBluetoothRemoteGattServer>;
  disconnect(): void;
  getPrimaryService(service: string): Promise<BrowserBluetoothRemoteGattService>;
};

export type BrowserBluetoothRemoteGattService = {
  getCharacteristic(characteristic: string): Promise<BrowserBluetoothRemoteGattCharacteristic>;
};

export type BrowserBluetoothRemoteGattCharacteristic = EventTarget & {
  readonly properties: BrowserBluetoothCharacteristicProperties;
  readonly value?: DataView;
  startNotifications(): Promise<BrowserBluetoothRemoteGattCharacteristic>;
  writeValue?(value: BufferSource): Promise<void>;
  writeValueWithResponse?(value: BufferSource): Promise<void>;
  writeValueWithoutResponse?(value: BufferSource): Promise<void>;
};

export type BrowserBluetoothCharacteristicProperties = {
  readonly write: boolean;
  readonly writeWithoutResponse: boolean;
};

export type BrowserBluetoothCharacteristicEvent = Event & {
  target: BrowserBluetoothRemoteGattCharacteristic | null;
};

export type BrowserUsb = {
  requestDevice(options: BrowserUsbRequestOptions): Promise<BrowserUsbDevice>;
};

export type BrowserUsbRequestOptions = {
  filters: readonly BrowserUsbDeviceFilter[];
};

export type BrowserUsbDeviceFilter = {
  vendorId?: number;
  productId?: number;
  classCode?: number;
  subclassCode?: number;
  protocolCode?: number;
  serialNumber?: string;
};

export type BrowserUsbDevice = {
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

export type BrowserUsbConfiguration = {
  readonly configurationValue: number;
  readonly interfaces: readonly BrowserUsbInterface[];
};

export type BrowserUsbInterface = {
  readonly interfaceNumber: number;
  readonly alternates: readonly BrowserUsbAlternateInterface[];
  readonly claimed?: boolean;
};

export type BrowserUsbAlternateInterface = {
  readonly alternateSetting: number;
  readonly interfaceClass?: number;
  readonly interfaceSubclass?: number;
  readonly interfaceProtocol?: number;
  readonly endpoints: readonly BrowserUsbEndpoint[];
};

export type BrowserUsbEndpoint = {
  readonly endpointNumber: number;
  readonly direction: "in" | "out";
  readonly type: "bulk" | "interrupt" | "isochronous";
  readonly packetSize: number;
};

export type BrowserUsbInTransferResult = {
  readonly data?: DataView;
  readonly status: "ok" | "stall" | "babble";
};

export type BrowserUsbOutTransferResult = {
  readonly bytesWritten: number;
  readonly status: "ok" | "stall";
};

export function hostGlobal(): HostGlobal {
  return globalThis as HostGlobal;
}
