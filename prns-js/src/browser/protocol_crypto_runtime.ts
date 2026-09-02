import { Tag } from "../casework.js";

type OwnedBytes = Uint8Array<ArrayBuffer>;

export type ProtocolCryptoJob =
  | Tag<
      "AnnounceVerify",
      {
        readonly id: number;
        readonly publicKey: OwnedBytes;
        readonly message: OwnedBytes;
        readonly signature: OwnedBytes;
      }
    >
  | Tag<
      "LinkProofVerify",
      {
        readonly id: number;
        readonly publicKey: OwnedBytes;
        readonly message: OwnedBytes;
        readonly signature: OwnedBytes;
        readonly secretScalar: OwnedBytes;
        readonly peerPublicKey: OwnedBytes;
      }
    >;

export function parseProtocolCryptoJob(raw: unknown): ProtocolCryptoJob | undefined {
  if (raw === undefined) {
    return undefined;
  }
  const root = record(raw, "protocol crypto job");
  const tag = stringField(root, "tag");
  const data = record(root.data, "protocol crypto job data");
  const common = {
    id: positiveIntegerField(data, "id"),
    publicKey: bytesField(data, "publicKey", 32),
    message: bytesField(data, "message"),
    signature: bytesField(data, "signature", 64),
  };
  if (tag === "AnnounceVerify") {
    return Tag("AnnounceVerify", common);
  }
  if (tag === "LinkProofVerify") {
    return Tag("LinkProofVerify", {
      ...common,
      secretScalar: bytesField(data, "secretScalar", 32),
      peerPublicKey: bytesField(data, "peerPublicKey", 32),
    });
  }
  throw new TypeError(`unknown protocol crypto job tag ${tag}`);
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null) {
    throw new TypeError(`${name} must be an object`);
  }
  return value as Record<string, unknown>;
}

function stringField(value: Record<string, unknown>, key: string): string {
  const field = value[key];
  if (typeof field !== "string") {
    throw new TypeError(`${key} must be a string`);
  }
  return field;
}

function positiveIntegerField(value: Record<string, unknown>, key: string): number {
  const field = value[key];
  if (!Number.isSafeInteger(field) || (field as number) < 1) {
    throw new TypeError(`${key} must be a positive safe integer`);
  }
  return field as number;
}

function bytesField(
  value: Record<string, unknown>,
  key: string,
  length?: number,
): OwnedBytes {
  const field = value[key];
  if (!(field instanceof Uint8Array) || !(field.buffer instanceof ArrayBuffer)) {
    throw new TypeError(`${key} must be an owned Uint8Array`);
  }
  if (length !== undefined && field.length !== length) {
    throw new TypeError(`${key} must be exactly ${length} bytes`);
  }
  return field as OwnedBytes;
}
