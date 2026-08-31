import type { Tag } from "../../casework.js";
import type { InterfaceId } from "../../contract.js";
import type { AlreadyActive } from "../interface_contract.js";
import type { InterfaceOutboundHost } from "../outbound.js";
import type {
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

type BluetoothHostOutcome<Outcome> = Outcome | Promise<Outcome>;

export type BluetoothHostReassembler = {
  absorb(
    bytes: Uint8Array,
  ): BluetoothHostOutcome<Uint8Array | undefined | RuntimeRejected>;
  release?(): void;
};

export type BluetoothRuntimeHost = InterfaceOutboundHost & {
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
  createBluetoothReassembler(): BluetoothHostReassembler;
  registerInterface(
    registration: BluetoothRuntimeRegistration,
  ): BluetoothHostOutcome<BluetoothRegistrationOutcome>;
  deactivateInterface(id: InterfaceId): BluetoothHostOutcome<BluetoothDetachOutcome>;
  ingest(id: InterfaceId, bytes: PacketFrame): BluetoothHostOutcome<BluetoothIngestOutcome>;
};
