import type {
  IdentitySecretKey,
  InterfaceId,
  PacketFrame,
  BluetoothReassemblerBinding,
  PrnsRuntimeBinding,
  UsbAutoDecoderBinding,
} from "../ts/index.js";

declare module "/pkg/personal_rns_wasm.js" {
  export class PrnsRuntime implements PrnsRuntimeBinding {
    constructor(identitySecretKey: IdentitySecretKey);
    registerInterface: PrnsRuntimeBinding["registerInterface"];
    bluetoothIdentity: PrnsRuntimeBinding["bluetoothIdentity"];
    registerSingleDestination: PrnsRuntimeBinding["registerSingleDestination"];
    announce: PrnsRuntimeBinding["announce"];
    ingest: PrnsRuntimeBinding["ingest"];
    drainEvents: PrnsRuntimeBinding["drainEvents"];
    drainOutbound: PrnsRuntimeBinding["drainOutbound"];
    snapshot: PrnsRuntimeBinding["snapshot"];
  }

  export class UsbAutoDecoder implements UsbAutoDecoderBinding {
    constructor();
    feed: UsbAutoDecoderBinding["feed"];
  }

  export class BluetoothReassembler implements BluetoothReassemblerBinding {
    constructor();
    absorb: BluetoothReassemblerBinding["absorb"];
  }

  export function identitySecretKeyLength(): number;
  export function interfaceIdLength(): number;
  export function destinationHashLength(): number;
  export function bluetoothServiceUuid(): string;
  export function bluetoothControlUuid(): string;
  export function bluetoothDataUuid(): string;
  export function bluetoothBitrateBps(): number;
  export function bluetoothHardwareMtu(): number;
  export function bluetoothDialerHello(identity: Uint8Array): Uint8Array;
  export function bluetoothDecodeControl(bytes: Uint8Array): unknown;
  export function bluetoothDataFragments(packet: PacketFrame): Uint8Array[];
  export function websocketBitrateBps(): number;
  export function websocketHardwareMtu(): number;
  export function usbAutoHostBitrateBps(): number;
  export function usbAutoHostHardwareMtu(): number;
  export function usbAutoWebUsbVendorId(): number;
  export function usbAutoWebUsbProductId(): number;
  export function usbAutoNodeTagFor(interfaceId: InterfaceId): Uint8Array;
  export function usbAutoHostHelloFrame(): Uint8Array;
  export function usbAutoHostHelloAckFrame(nodeTag: Uint8Array): Uint8Array;
  export function usbAutoDataFrame(packet: PacketFrame): Uint8Array;

  export default function init(moduleOrPath?: unknown): Promise<unknown>;
}
