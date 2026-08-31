import { identityHash, interfaceId, linkId } from "../contract.js";
import type { InterfaceSnapshot } from "./snapshot.js";
import type { ActiveLinkSnapshot } from "./projections.js";
import type { RouteSnapshot } from "../contract.js";
import {
  bytesField,
  nonNegativeBigIntField,
  numberField,
  optionalArrayField,
  optionalBytesField,
  record,
  stringField,
} from "./decoding.js";
import { destinationHash } from "../contract.js";
import { nonNegativeInteger, PrnsValidationError } from "./values.js";

export type RuntimeProjectionSnapshot = {
  readonly revision: bigint;
  readonly interfaces?: readonly InterfaceSnapshot[];
  readonly routes?: readonly RouteSnapshot[];
  readonly links?: readonly ActiveLinkSnapshot[];
};

export function parseRuntimeProjectionSnapshot(
  raw: unknown,
): RuntimeProjectionSnapshot {
  const object = record(raw, "RuntimeProjectionSnapshot");
  const interfaces = optionalArrayField(object, "interfaces");
  const routes = optionalArrayField(object, "routes");
  const links = optionalArrayField(object, "links");
  return {
    revision: nonNegativeBigIntField(object, "revision"),
    ...(fieldIsPresent(object, "interfaces")
      ? { interfaces: interfaces.map(parseInterface) }
      : {}),
    ...(fieldIsPresent(object, "routes")
      ? { routes: routes.map(parseRoute) }
      : {}),
    ...(fieldIsPresent(object, "links")
      ? { links: links.map(parseLink) }
      : {}),
  };
}

function parseInterface(raw: unknown): InterfaceSnapshot {
  const object = record(raw, "ProjectionInterfaceSnapshot");
  return {
    id: interfaceId(bytesField(object, "id")),
    kind: stringField(object, "kind"),
    routes: nonNegativeInteger(numberField(object, "routes"), "routes"),
    links: nonNegativeInteger(numberField(object, "links"), "links"),
    transportedLinks: nonNegativeInteger(
      numberField(object, "transportedLinks"),
      "transportedLinks",
    ),
  };
}

function parseRoute(raw: unknown): RouteSnapshot {
  const object = record(raw, "ProjectionRouteSnapshot");
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
    lastRouteActivityAtMillis: nonNegativeInteger(
      numberField(object, "lastRouteActivityAtMillis"),
      "lastRouteActivityAtMillis",
    ),
    expiresAtMillis: nonNegativeInteger(
      numberField(object, "expiresAtMillis"),
      "expiresAtMillis",
    ),
  };
}

function parseLink(raw: unknown): ActiveLinkSnapshot {
  const object = record(raw, "ProjectionLinkSnapshot");
  const peerIdentity = optionalBytesField(object, "peerIdentity");
  return {
    linkId: linkId(bytesField(object, "linkId")),
    rttMillis: nonNegativeInteger(
      numberField(object, "rttMillis"),
      "rttMillis",
    ),
    ...(peerIdentity === undefined
      ? {}
      : { peerIdentity: identityHash(peerIdentity) }),
  };
}

function fieldIsPresent(object: Record<string, unknown>, name: string): boolean {
  const value = object[name];
  if (value === undefined) {
    return false;
  }
  if (!Array.isArray(value)) {
    throw new PrnsValidationError("invalid-component", `${name} must be an array`);
  }
  return true;
}
