import type {
  BluetoothReassemblerBinding,
  IdentitySecretKey,
  InterfaceId,
  PacketFrame,
  PrnsRuntimeBinding,
  UsbAutoDecoderBinding,
} from "./index.js";

export declare class PrnsRuntime implements PrnsRuntimeBinding {
  constructor(identitySecretKey: IdentitySecretKey, bleIdentity?: Uint8Array);
  registerInterface: PrnsRuntimeBinding["registerInterface"];
  removeInterface: PrnsRuntimeBinding["removeInterface"];
  bluetoothIdentity: PrnsRuntimeBinding["bluetoothIdentity"];
  registerSingleDestination: PrnsRuntimeBinding["registerSingleDestination"];
  announce: PrnsRuntimeBinding["announce"];
  ingest: PrnsRuntimeBinding["ingest"];
  drainEvents: PrnsRuntimeBinding["drainEvents"];
  drainOutbound: PrnsRuntimeBinding["drainOutbound"];
  snapshot: PrnsRuntimeBinding["snapshot"];
}

export declare class UsbAutoDecoder implements UsbAutoDecoderBinding {
  constructor();
  feed: UsbAutoDecoderBinding["feed"];
}

export declare class BluetoothReassembler
  implements BluetoothReassemblerBinding
{
  constructor();
  absorb: BluetoothReassemblerBinding["absorb"];
}

export declare function identitySecretKeyLength(): number;
export declare function interfaceIdLength(): number;
export declare function destinationHashLength(): number;
export declare function bluetoothServiceUuid(): string;
export declare function bluetoothControlUuid(): string;
export declare function bluetoothDataUuid(): string;
export declare function bluetoothBitrateBps(): number;
export declare function bluetoothHardwareMtu(): number;
export declare function bluetoothDialerHello(identity: Uint8Array): Uint8Array;
export declare function bluetoothDecodeControl(bytes: Uint8Array): unknown;
export declare function bluetoothDataFragments(
  packet: PacketFrame,
): Uint8Array[];
export declare function websocketBitrateBps(): number;
export declare function websocketFrameCap(): number;
export declare function websocketHardwareMtu(): number;
export declare function usbAutoHostBitrateBps(): number;
export declare function usbAutoHostHardwareMtu(): number;
export declare function usbAutoWebUsbVendorId(): number;
export declare function usbAutoWebUsbProductId(): number;
export declare function usbAutoNodeTagFor(
  interfaceId: InterfaceId,
): Uint8Array;
export declare function usbAutoHostHelloFrame(): Uint8Array;
export declare function usbAutoHostHelloAckFrame(
  nodeTag: Uint8Array,
): Uint8Array;
export declare function usbAutoDataFrame(packet: PacketFrame): Uint8Array;
export default function init(moduleOrPath?: unknown): Promise<unknown>;
