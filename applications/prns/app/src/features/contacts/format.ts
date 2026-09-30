import {
  destinationHash,
  identityHash,
  type DestinationHash,
  type IdentityHash,
} from "personal-rns/contract";

const hashHex = /^[0-9a-fA-F]{32}$/;

export function formatContactHash(value: Uint8Array): string {
  return Array.from(value, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function parseDestinationHash(value: string): DestinationHash | null {
  const bytes = parseHashBytes(value);
  return bytes === null ? null : destinationHash(bytes);
}

export function parseIdentityHash(value: string): IdentityHash | null {
  const bytes = parseHashBytes(value);
  return bytes === null ? null : identityHash(bytes);
}

function parseHashBytes(value: string): Uint8Array | null {
  const normalized = value.trim();
  if (!hashHex.test(normalized)) {
    return null;
  }
  return Uint8Array.from(normalized.match(/../g)?.map((pair) => Number.parseInt(pair, 16)) ?? []);
}
