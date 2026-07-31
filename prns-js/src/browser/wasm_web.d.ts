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
  registerNodePage: PrnsRuntimeBinding["registerNodePage"];
  announce: PrnsRuntimeBinding["announce"];
  sendSinglePacket: PrnsRuntimeBinding["sendSinglePacket"];
  establishLink: PrnsRuntimeBinding["establishLink"];
  requestPath: PrnsRuntimeBinding["requestPath"];
  identify: PrnsRuntimeBinding["identify"];
  sendLinkPacket: PrnsRuntimeBinding["sendLinkPacket"];
  request: PrnsRuntimeBinding["request"];
  respond: PrnsRuntimeBinding["respond"];
  resourceSegmentPlan: PrnsRuntimeBinding["resourceSegmentPlan"];
  sendResourceSegment: PrnsRuntimeBinding["sendResourceSegment"];
  setLinkResourceStrategy: PrnsRuntimeBinding["setLinkResourceStrategy"];
  setDestinationResourceStrategy: PrnsRuntimeBinding["setDestinationResourceStrategy"];
  sendChannelMessage: PrnsRuntimeBinding["sendChannelMessage"];
  allowRequester: PrnsRuntimeBinding["allowRequester"];
  closeLink: PrnsRuntimeBinding["closeLink"];
  ingest: PrnsRuntimeBinding["ingest"];
  drainEvents: PrnsRuntimeBinding["drainEvents"];
  drainOutbound: PrnsRuntimeBinding["drainOutbound"];
  persistedState: PrnsRuntimeBinding["persistedState"];
  restorePersistedState: PrnsRuntimeBinding["restorePersistedState"];
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

export declare function hostContractAbi(): number;
export declare function hostSchemaVersion(): number;
export declare function browserPersistenceVersion(): number;
export declare function productVersion(): string;
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
export declare function compressResourceCandidate(options: {
  readonly payload: Uint8Array;
  readonly packedMetadata?: Uint8Array;
}): Uint8Array | undefined;
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
