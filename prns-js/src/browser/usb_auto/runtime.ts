import type { Tag } from "../../casework.js";
import type { InterfaceId } from "../../contract.js";
import type { BrowserUsbDeviceFilter } from "../host_apis.js";
import type {
  AlreadyActive,
  InterfaceSessionFailure,
} from "../interface_contract.js";
import type { PrnsOutboundFrame } from "../outbound.js";
import type {
  EntropyFailure,
  RuntimeRejected,
  UsbAutoDecoderBinding,
} from "../runtime_contract.js";
import type {
  BitrateBps,
  ChannelTag,
  HardwareMtu,
  PacketFrame,
} from "../values.js";

export type UsbAutoRuntimeRegistration = {
  readonly interfaceName: "usb-auto";
  readonly kind: "auto-usb-host";
  readonly channelTag: ChannelTag;
  readonly bitrateBps: BitrateBps;
  readonly hardwareMtu: HardwareMtu;
};

type UsbAutoRegistrationOutcome =
  | Tag<"Registered", InterfaceId>
  | AlreadyActive<"usb-auto">
  | RuntimeRejected;

type UsbAutoDetachOutcome = Tag<"Detached"> | RuntimeRejected;

type UsbAutoIngestOutcome =
  | Tag<"Accepted">
  | EntropyFailure
  | RuntimeRejected;

type UsbAutoOutboundOutcome =
  | Tag<"Outbound", readonly PrnsOutboundFrame[]>
  | Extract<InterfaceSessionFailure, Tag<"OutboundQueueFull", unknown>>
  | RuntimeRejected;

export type UsbAutoRuntimeHost = {
  runtimeReadiness(): Tag<"Ready"> | RuntimeRejected;
  defaultUsbAutoFilters(): readonly BrowserUsbDeviceFilter[];
  usbAutoHostBitrateBps(): BitrateBps;
  usbAutoHostHardwareMtu(): HardwareMtu;
  usbAutoNodeTagFor(interfaceId: InterfaceId): Uint8Array;
  usbAutoHostHelloFrame(): Uint8Array;
  usbAutoHostHelloAckFrame(nodeTag: Uint8Array): Uint8Array;
  usbAutoDataFrame(packet: PacketFrame): Uint8Array;
  createUsbAutoDecoder(): UsbAutoDecoderBinding;
  registerInterface(
    registration: UsbAutoRuntimeRegistration,
  ): UsbAutoRegistrationOutcome;
  deactivateInterface(id: InterfaceId): UsbAutoDetachOutcome;
  ingest(id: InterfaceId, bytes: PacketFrame): UsbAutoIngestOutcome;
  takeOutboundFor(id: InterfaceId): UsbAutoOutboundOutcome;
};
