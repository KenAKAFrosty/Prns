import { INTERFACE_ID_LENGTH } from "../contract.js";
import type { InterfaceId } from "../contract.js";

declare const interfaceKeyBrand: unique symbol;

export type InterfaceKey = string & {
  readonly [interfaceKeyBrand]: "InterfaceKey";
};

export function byteKey(bytes: Uint8Array): string {
  let key = "";
  for (const byte of bytes) {
    key += byte.toString(16).padStart(2, "0");
  }
  return key;
}

export function interfaceKey(id: InterfaceId): InterfaceKey {
  if (id.byteLength !== INTERFACE_ID_LENGTH) {
    throw new TypeError("interface key requires an InterfaceId");
  }
  return String.fromCharCode(
    id[0]! | (id[1]! << 8),
    id[2]! | (id[3]! << 8),
    id[4]! | (id[5]! << 8),
    id[6]! | (id[7]! << 8),
  ) as InterfaceKey;
}
