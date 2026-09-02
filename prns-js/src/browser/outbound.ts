import { Tag, match_into } from "../casework.js";
import { interfaceId } from "../contract.js";
import type { InterfaceId } from "../contract.js";
import type { TransferredByteBatch } from "./byte_transfer.js";
import {
  bytesField,
  field,
  optionalNumber,
  record,
  stringField,
} from "./decoding.js";
import { runtimeInterfaceKind } from "./interface_kind.js";
import type {
  RuntimeInterfaceKind,
  RuntimeRejected,
} from "./runtime_contract.js";
import {
  PrnsValidationError,
  hopCount,
  packetFrameView,
} from "./values.js";
import type { HopCount, PacketFrame } from "./values.js";

export type FanTarget =
  | Tag<"All">
  | Tag<"Only", InterfaceId>
  | Tag<"AllExcept", InterfaceId>;

export type OutboundTarget =
  | Tag<"Interface", InterfaceId>
  | Tag<
      "Broadcast",
      {
        readonly supervisorKind: RuntimeInterfaceKind;
        readonly fan: FanTarget;
      }
    >;

export type PrnsOutboundFrame = {
  type: "frame" | "announce";
  target: OutboundTarget;
  hops?: HopCount;
  bytes: PacketFrame;
};

export type NonEmptyPrnsOutboundFrames = readonly [
  PrnsOutboundFrame,
  ...PrnsOutboundFrame[],
];

export type InterfaceOutboundOutcome =
  | Tag<"Outbound", NonEmptyPrnsOutboundFrames>
  | Tag<"InterfaceDetached">
  | Tag<"OutboundQueueFull", { readonly capacity: number }>
  | RuntimeRejected;

export type TransferredInterfaceOutboundOutcome =
  | Tag<"TransferredOutbound", TransferredByteBatch>
  | Exclude<InterfaceOutboundOutcome, Tag<"Outbound", unknown>>;

export type InterfaceOutboundHost = {
  nextOutboundFor(
    interfaceId: InterfaceId,
    maximumFrames?: number,
  ): Promise<InterfaceOutboundOutcome>;
};

export function outboundTargets(
  target: OutboundTarget,
  interfaceId: InterfaceId,
  supervisorKind: RuntimeInterfaceKind,
): boolean {
  return match_into<boolean>().from(target, {
    Interface: (targetInterface) =>
      equalBytes(targetInterface, interfaceId),
    Broadcast: ({ supervisorKind: targetKind, fan }) =>
      targetKind === supervisorKind &&
      match_into<boolean>().from(fan, {
        All: () => true,
        Only: (targetInterface) =>
          equalBytes(targetInterface, interfaceId),
        AllExcept: (targetInterface) =>
          !equalBytes(targetInterface, interfaceId),
      }),
  });
}

export function parseOutboundFrame(raw: unknown): PrnsOutboundFrame {
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
    bytes: packetFrameView(bytesField(object, "bytes")),
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
    return Tag(
      "Interface",
      interfaceId(bytesField(object, "interfaceId")),
    );
  }
  if (type === "broadcast") {
    return Tag("Broadcast", {
      supervisorKind: runtimeInterfaceKind(
        stringField(object, "supervisorKind"),
      ),
      fan: parseFanTarget(field(object, "fan")),
    });
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
    return Tag("All");
  }
  if (type === "only") {
    return Tag(
      "Only",
      interfaceId(bytesField(object, "interfaceId")),
    );
  }
  if (type === "allExcept") {
    return Tag(
      "AllExcept",
      interfaceId(bytesField(object, "interfaceId")),
    );
  }
  throw new PrnsValidationError(
    "unknown-outbound-target",
    `unknown fan target ${type}`,
  );
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
