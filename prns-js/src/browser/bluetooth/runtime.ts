import type { Tag } from "../../casework.js";
import type { InterfaceId } from "../../contract.js";
import type {
  AlreadyActive,
  InterfaceSessionFailure,
} from "../interface_contract.js";
import type { PrnsOutboundFrame } from "../outbound.js";
import type {
  BluetoothReassemblerBinding,
  EntropyFailure,
  RuntimeRejected,
  StableIdentityUnavailable,
} from "../runtime_contract.js";
import type {
  BitrateBps,
  ChannelTag,
  HardwareMtu,
  PacketFrame,
} from "../values.js";

export type BluetoothRuntimeRegistration = {
  readonly interfaceName: "bluetooth";
  readonly supervisorKind: "bluetooth-auto";
  readonly kind: "bluetooth-peer";
  readonly channelTag: ChannelTag;
  readonly bitrateBps: BitrateBps;
  readonly hardwareMtu: HardwareMtu;
};

type BluetoothRegistrationOutcome =
  | Tag<"Registered", InterfaceId>
  | AlreadyActive<"bluetooth">
  | RuntimeRejected;

type BluetoothDetachOutcome = Tag<"Detached"> | RuntimeRejected;

type BluetoothIngestOutcome =
  | Tag<"Accepted">
  | EntropyFailure
  | RuntimeRejected;

type BluetoothOutboundOutcome =
  | Tag<"Outbound", readonly PrnsOutboundFrame[]>
  | Extract<InterfaceSessionFailure, Tag<"OutboundQueueFull", unknown>>
  | RuntimeRejected;

type BluetoothOutboundActivityOutcome =
  | Tag<"RuntimeAdvanced">
  | Tag<"InterfaceDetached">;

export type BluetoothRuntimeHost = {
  bluetoothIdentityReadiness():
    | Tag<"Ready">
    | StableIdentityUnavailable<"bluetooth">;
  runtimeReadiness(): Tag<"Ready"> | RuntimeRejected;
  bluetoothServiceUuid(): string;
  bluetoothControlUuid(): string;
  bluetoothDataUuid(): string;
  bluetoothBitrateBps(): BitrateBps;
  bluetoothHardwareMtu(): HardwareMtu;
  bluetoothDialerHello(): Uint8Array;
  bluetoothDecodeControl(bytes: Uint8Array): unknown;
  bluetoothDataFragments(packet: PacketFrame): Uint8Array[];
  createBluetoothReassembler(): BluetoothReassemblerBinding;
  registerInterface(
    registration: BluetoothRuntimeRegistration,
  ): BluetoothRegistrationOutcome;
  deactivateInterface(id: InterfaceId): BluetoothDetachOutcome;
  ingest(id: InterfaceId, bytes: PacketFrame): BluetoothIngestOutcome;
  takeOutboundFor(id: InterfaceId): BluetoothOutboundOutcome;
  waitForOutboundActivity(
    id: InterfaceId,
  ): Promise<BluetoothOutboundActivityOutcome>;
};
