import {
  destinationHash,
  identityHash,
  interfaceId,
} from "../contract.js";
import type {
  DestinationIdentitySnapshot,
  InterfaceId,
  RouteSnapshot,
} from "../contract.js";
import {
  bytesField,
  field,
  literalField,
  nonNegativeBigIntField,
  numberField,
  optionalArrayField,
  optionalBytesField,
  optionalNumber,
  record,
  stringField,
} from "./decoding.js";
import {
  PrnsValidationError,
  bitrateBps,
  hardwareMtu,
  nonNegativeInteger,
} from "./values.js";
import type { BitrateBps, HardwareMtu } from "./values.js";

export type InterfaceSnapshot = {
  id: InterfaceId;
  kind: string;
  bitrateBps?: BitrateBps;
  hardwareMtu?: HardwareMtu;
  routes: number;
  links: number;
  transportedLinks: number;
};

export type PrnsSnapshot = {
  type: "snapshot";
  revision: bigint;
  ingestedPackets: number;
  ingestedCommands: number;
  routes: number;
  scheduledAnnounces: number;
  interfaces: InterfaceSnapshot[];
  activeLinkCount: number;
  routeSnapshots: RouteSnapshot[];
  destinationIdentities: DestinationIdentitySnapshot[];
};

export function parseSnapshot(raw: unknown): PrnsSnapshot {
  const object = record(raw, "PrnsSnapshot");
  const interfacesRaw = field(object, "interfaces");
  if (!Array.isArray(interfacesRaw)) {
    throw new PrnsValidationError(
      "invalid-component",
      "snapshot interfaces must be an array",
    );
  }
  const routeSnapshotsRaw = optionalArrayField(object, "routeSnapshots");
  const destinationIdentitiesRaw = optionalArrayField(
    object,
    "destinationIdentities",
  );
  return {
    type: literalField(object, "type", "snapshot"),
    revision: nonNegativeBigIntField(object, "revision"),
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
    activeLinkCount: optionalNumber(
      object,
      "activeLinkCount",
      (value) => nonNegativeInteger(value, "activeLinkCount"),
    ) ?? 0,
    routeSnapshots: routeSnapshotsRaw.map(parseRouteSnapshot),
    destinationIdentities: destinationIdentitiesRaw.map(
      parseDestinationIdentitySnapshot,
    ),
  };
}

function parseInterfaceSnapshot(raw: unknown): InterfaceSnapshot {
  const object = record(raw, "InterfaceSnapshot");
  const snapshot: InterfaceSnapshot = {
    id: interfaceId(bytesField(object, "id")),
    kind: stringField(object, "kind"),
    routes: nonNegativeInteger(numberField(object, "routes"), "routes"),
    links: nonNegativeInteger(numberField(object, "links"), "links"),
    transportedLinks: optionalNumber(
      object,
      "transportedLinks",
      (value) => nonNegativeInteger(value, "transportedLinks"),
    ) ?? 0,
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

function parseRouteSnapshot(raw: unknown): RouteSnapshot {
  const object = record(raw, "RouteSnapshot");
  const viaIdentity = optionalBytesField(object, "viaIdentity");
  return {
    destination: destinationHash(bytesField(object, "destination")),
    hops: nonNegativeInteger(numberField(object, "hops"), "hops"),
    ...(viaIdentity === undefined
      ? {}
      : { viaIdentity: identityHash(viaIdentity) }),
    interfaceId: interfaceId(bytesField(object, "interfaceId")),
    learnedAtMillis: nonNegativeInteger(
      numberField(object, "learnedAtMillis"),
      "learnedAtMillis",
    ),
    lastRelayedAtMillis: nonNegativeInteger(
      numberField(object, "lastRelayedAtMillis"),
      "lastRelayedAtMillis",
    ),
    expiresAtMillis: nonNegativeInteger(
      numberField(object, "expiresAtMillis"),
      "expiresAtMillis",
    ),
  };
}

function parseDestinationIdentitySnapshot(
  raw: unknown,
): DestinationIdentitySnapshot {
  const object = record(raw, "DestinationIdentitySnapshot");
  return {
    destination: destinationHash(bytesField(object, "destination")),
    identity: identityHash(bytesField(object, "identity")),
  };
}
