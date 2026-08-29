import {
  DESTINATION_HASH_LENGTH,
  Tag,
  appData,
  destinationHash,
} from "personal-rns/browser";
import type {
  AppData,
  DestinationHash,
  Tag as Tagged,
} from "personal-rns/browser";
import type { StoredDropContact } from "./model.js";

const PROFILE_MAGIC = new Uint8Array([0x50, 0x44, 0x50, 0x01]);
const MESSAGE_MAGIC = new Uint8Array([0x50, 0x44, 0x4d, 0x01]);
const TEXT_MESSAGE_KIND = 1;
const MESSAGE_ID_LENGTH = 16;
const TIMESTAMP_LENGTH = 8;
const DISPLAY_NAME_LENGTH_FIELD = 1;
const TEXT_LENGTH_FIELD = 2;
const MESSAGE_FIXED_LENGTH =
  MESSAGE_MAGIC.length +
  1 +
  MESSAGE_ID_LENGTH +
  DESTINATION_HASH_LENGTH +
  TIMESTAMP_LENGTH +
  DISPLAY_NAME_LENGTH_FIELD +
  TEXT_LENGTH_FIELD;
const CONTACT_CODE_PREFIX = "prns-drop:v1:";
const MAX_DATE_TIMESTAMP = 8_640_000_000_000_000n;
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

export const MAX_DROP_DISPLAY_NAME_BYTES = 48;
export const MAX_DROP_TEXT_BYTES = 280;

export type PreparedDropProfile = {
  readonly displayName: string;
  readonly appData: AppData;
};

export type DropProfileFailure =
  | Tagged<"DisplayNameEmpty">
  | Tagged<
      "DisplayNameTooLong",
      { readonly actualBytes: number; readonly maximumBytes: number }
    >;

export type PrepareDropProfileOutcome =
  | Tagged<"Prepared", PreparedDropProfile>
  | DropProfileFailure;

export type DecodeDropProfileOutcome =
  | Tagged<"Decoded", { readonly displayName: string }>
  | Tagged<"NotDropProfile">
  | Tagged<"MalformedDropProfile", { readonly detail: string }>;

export type DropWireTextMessage = {
  readonly id: Uint8Array;
  readonly idHex: string;
  readonly sender: DestinationHash;
  readonly senderHex: string;
  readonly senderDisplayName: string;
  readonly sentAt: number;
  readonly text: string;
};

export type EncodeDropTextOutcome =
  | Tagged<"Encoded", Uint8Array>
  | DropProfileFailure
  | Tagged<"MessageIdInvalid", { readonly actualBytes: number }>
  | Tagged<"TimestampInvalid", { readonly value: number }>
  | Tagged<"TextEmpty">
  | Tagged<
      "TextTooLong",
      { readonly actualBytes: number; readonly maximumBytes: number }
    >;

export type DecodeDropTextOutcome =
  | Tagged<"Decoded", DropWireTextMessage>
  | Tagged<"NotDropMessage">
  | Tagged<"UnsupportedDropMessage", { readonly kind: number }>
  | Tagged<"MalformedDropMessage", { readonly detail: string }>;

export type ParseDropContactCodeOutcome =
  | Tagged<"Parsed", StoredDropContact>
  | Tagged<"InvalidContactCode", { readonly detail: string }>;

export function prepareDropProfile(displayName: string): PrepareDropProfileOutcome {
  const normalized = displayName.trim();
  if (normalized.length === 0) {
    return Tag("DisplayNameEmpty");
  }
  const nameBytes = encoder.encode(normalized);
  if (nameBytes.length > MAX_DROP_DISPLAY_NAME_BYTES) {
    return Tag("DisplayNameTooLong", {
      actualBytes: nameBytes.length,
      maximumBytes: MAX_DROP_DISPLAY_NAME_BYTES,
    });
  }
  const encoded = new Uint8Array(PROFILE_MAGIC.length + 1 + nameBytes.length);
  encoded.set(PROFILE_MAGIC);
  encoded[PROFILE_MAGIC.length] = nameBytes.length;
  encoded.set(nameBytes, PROFILE_MAGIC.length + 1);
  return Tag("Prepared", {
    displayName: normalized,
    appData: appData(encoded),
  });
}

export function decodeDropProfile(bytes: Uint8Array): DecodeDropProfileOutcome {
  if (!startsWith(bytes, PROFILE_MAGIC)) {
    return Tag("NotDropProfile");
  }
  if (bytes.length < PROFILE_MAGIC.length + 1) {
    return Tag("MalformedDropProfile", { detail: "profile length field is missing" });
  }
  const nameLength = bytes[PROFILE_MAGIC.length];
  if (nameLength === undefined || nameLength === 0) {
    return Tag("MalformedDropProfile", { detail: "display name is empty" });
  }
  if (nameLength > MAX_DROP_DISPLAY_NAME_BYTES) {
    return Tag("MalformedDropProfile", { detail: "display name exceeds the protocol limit" });
  }
  if (bytes.length !== PROFILE_MAGIC.length + 1 + nameLength) {
    return Tag("MalformedDropProfile", { detail: "profile length does not match its payload" });
  }
  try {
    return Tag("Decoded", {
      displayName: decoder.decode(bytes.subarray(PROFILE_MAGIC.length + 1)),
    });
  } catch (error: unknown) {
    return Tag("MalformedDropProfile", { detail: describeUnknown(error) });
  }
}

export function encodeDropTextMessage(input: {
  readonly id: Uint8Array;
  readonly sender: DestinationHash;
  readonly senderDisplayName: string;
  readonly sentAt: number;
  readonly text: string;
}): EncodeDropTextOutcome {
  if (input.id.length !== MESSAGE_ID_LENGTH) {
    return Tag("MessageIdInvalid", { actualBytes: input.id.length });
  }
  if (
    !Number.isSafeInteger(input.sentAt) ||
    input.sentAt < 0 ||
    input.sentAt > Number(MAX_DATE_TIMESTAMP)
  ) {
    return Tag("TimestampInvalid", { value: input.sentAt });
  }
  const profile = prepareDropProfile(input.senderDisplayName);
  if (profile.tag !== "Prepared") {
    return profile;
  }
  if (input.text.trim().length === 0) {
    return Tag("TextEmpty");
  }
  const nameBytes = encoder.encode(profile.data.displayName);
  const textBytes = encoder.encode(input.text);
  if (textBytes.length > MAX_DROP_TEXT_BYTES) {
    return Tag("TextTooLong", {
      actualBytes: textBytes.length,
      maximumBytes: MAX_DROP_TEXT_BYTES,
    });
  }
  const encoded = new Uint8Array(
    MESSAGE_FIXED_LENGTH + nameBytes.length + textBytes.length,
  );
  const view = new DataView(encoded.buffer);
  let offset = 0;
  encoded.set(MESSAGE_MAGIC, offset);
  offset += MESSAGE_MAGIC.length;
  encoded[offset++] = TEXT_MESSAGE_KIND;
  encoded.set(input.id, offset);
  offset += MESSAGE_ID_LENGTH;
  encoded.set(input.sender, offset);
  offset += DESTINATION_HASH_LENGTH;
  view.setBigUint64(offset, BigInt(input.sentAt), false);
  offset += TIMESTAMP_LENGTH;
  encoded[offset++] = nameBytes.length;
  view.setUint16(offset, textBytes.length, false);
  offset += TEXT_LENGTH_FIELD;
  encoded.set(nameBytes, offset);
  offset += nameBytes.length;
  encoded.set(textBytes, offset);
  return Tag("Encoded", encoded);
}

export function decodeDropTextMessage(bytes: Uint8Array): DecodeDropTextOutcome {
  if (!startsWith(bytes, MESSAGE_MAGIC)) {
    return Tag("NotDropMessage");
  }
  if (bytes.length < MESSAGE_FIXED_LENGTH) {
    return Tag("MalformedDropMessage", { detail: "message header is truncated" });
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let offset = MESSAGE_MAGIC.length;
  const kind = bytes[offset++];
  if (kind !== TEXT_MESSAGE_KIND) {
    return Tag("UnsupportedDropMessage", { kind: kind ?? -1 });
  }
  const id = bytes.slice(offset, offset + MESSAGE_ID_LENGTH);
  offset += MESSAGE_ID_LENGTH;
  const sender = destinationHash(bytes.slice(offset, offset + DESTINATION_HASH_LENGTH));
  offset += DESTINATION_HASH_LENGTH;
  const sentAtValue = view.getBigUint64(offset, false);
  offset += TIMESTAMP_LENGTH;
  if (sentAtValue > MAX_DATE_TIMESTAMP) {
    return Tag("MalformedDropMessage", { detail: "message timestamp is outside the Date range" });
  }
  const nameLength = bytes[offset++];
  const textLength = view.getUint16(offset, false);
  offset += TEXT_LENGTH_FIELD;
  if (nameLength === undefined || nameLength === 0 || nameLength > MAX_DROP_DISPLAY_NAME_BYTES) {
    return Tag("MalformedDropMessage", { detail: "sender display name length is invalid" });
  }
  if (textLength === 0 || textLength > MAX_DROP_TEXT_BYTES) {
    return Tag("MalformedDropMessage", { detail: "message text length is invalid" });
  }
  if (offset + nameLength + textLength !== bytes.length) {
    return Tag("MalformedDropMessage", { detail: "message length does not match its payload" });
  }
  try {
    const senderDisplayName = decoder.decode(bytes.subarray(offset, offset + nameLength));
    offset += nameLength;
    const text = decoder.decode(bytes.subarray(offset));
    return Tag("Decoded", {
      id,
      idHex: hex(id),
      sender,
      senderHex: hex(sender),
      senderDisplayName,
      sentAt: Number(sentAtValue),
      text,
    });
  } catch (error: unknown) {
    return Tag("MalformedDropMessage", { detail: describeUnknown(error) });
  }
}

export function exportDropContactCode(contact: {
  readonly destinationHex: string;
  readonly displayName: string;
}): string {
  return `${CONTACT_CODE_PREFIX}${contact.destinationHex}:${encodeURIComponent(contact.displayName)}`;
}

export function parseDropContactCode(value: string): ParseDropContactCodeOutcome {
  const selected = value.trim();
  if (!selected.startsWith(CONTACT_CODE_PREFIX)) {
    return Tag("InvalidContactCode", { detail: "contact code prefix is not recognized" });
  }
  const remainder = selected.slice(CONTACT_CODE_PREFIX.length);
  const separator = remainder.indexOf(":");
  if (separator < 0) {
    return Tag("InvalidContactCode", { detail: "contact code is missing its display name" });
  }
  const destinationHex = remainder.slice(0, separator).toLowerCase();
  const parsedDestination = destinationFromHex(destinationHex);
  if (parsedDestination.tag !== "Parsed") {
    return parsedDestination;
  }
  let displayName: string;
  try {
    displayName = decodeURIComponent(remainder.slice(separator + 1));
  } catch (error: unknown) {
    return Tag("InvalidContactCode", { detail: describeUnknown(error) });
  }
  const profile = prepareDropProfile(displayName);
  if (profile.tag !== "Prepared") {
    return Tag("InvalidContactCode", {
      detail: profile.tag === "DisplayNameEmpty"
        ? "contact display name is empty"
        : `contact display name exceeds ${profile.data.maximumBytes} bytes`,
    });
  }
  return Tag("Parsed", {
    destination: parsedDestination.data,
    destinationHex,
    displayName: profile.data.displayName,
  });
}

export function destinationFromHex(value: string):
  | Tagged<"Parsed", DestinationHash>
  | Tagged<"InvalidContactCode", { readonly detail: string }> {
  if (value.length !== DESTINATION_HASH_LENGTH * 2 || !/^[0-9a-f]+$/i.test(value)) {
    return Tag("InvalidContactCode", {
      detail: `destination must be ${DESTINATION_HASH_LENGTH * 2} hexadecimal characters`,
    });
  }
  const bytes = new Uint8Array(DESTINATION_HASH_LENGTH);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(value.slice(index * 2, index * 2 + 2), 16);
  }
  return Tag("Parsed", destinationHash(bytes));
}

export function hex(bytes: Uint8Array): string {
  let value = "";
  for (const byte of bytes) {
    value += byte.toString(16).padStart(2, "0");
  }
  return value;
}

function startsWith(value: Uint8Array, prefix: Uint8Array): boolean {
  if (value.length < prefix.length) {
    return false;
  }
  for (let index = 0; index < prefix.length; index += 1) {
    if (value[index] !== prefix[index]) {
      return false;
    }
  }
  return true;
}

function describeUnknown(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
