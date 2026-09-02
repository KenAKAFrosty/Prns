import { Tag } from "../casework.js";

declare const brand: unique symbol;

type Brand<Name extends string> = { readonly [brand]: Name };
type BrandedBytes<Name extends string> = Uint8Array & Brand<Name>;
type BrandedNumber<Name extends string> = number & Brand<Name>;
type BrandedBigInt<Name extends string> = bigint & Brand<Name>;

export const MIN_ENTROPY_BYTES = 128;
export const BLE_IDENTITY_LENGTH = 16;

export type IdentitySecretKey = BrandedBytes<"IdentitySecretKey">;
export type BleIdentity = BrandedBytes<"BleIdentity">;
export type ChannelTag = BrandedBytes<"ChannelTag">;
export type PacketFrame = BrandedBytes<"PacketFrame">;
export type EntropyBytes = BrandedBytes<"EntropyBytes">;
export type AppData = BrandedBytes<"AppData">;

export type AppName = string & Brand<"AppName">;
export type Aspect = string & Brand<"Aspect">;
export type InstantMillis = BrandedNumber<"InstantMillis">;
export type BitrateBps = BrandedNumber<"BitrateBps">;
export type HardwareMtu = BrandedNumber<"HardwareMtu">;
export type HopCount = BrandedNumber<"HopCount">;
export type CommandId = BrandedBigInt<"CommandId">;

export type PrnsValidationCode =
  | "empty-bytes"
  | "empty-string"
  | "invalid-component"
  | "invalid-length"
  | "invalid-number"
  | "missing-host-api"
  | "unknown-interface-kind"
  | "unknown-outbound-target";

export class PrnsValidationError extends Error {
  readonly code: PrnsValidationCode;

  constructor(code: PrnsValidationCode, message: string) {
    super(message);
    this.name = "PrnsValidationError";
    this.code = code;
  }
}

export type BleIdentityValidationOutcome =
  | Tag<"ValidBleIdentity", BleIdentity>
  | Tag<"InvalidBleIdentity", { readonly actualLength: number }>;

export function identitySecretKey(
  bytes: Uint8Array,
  expectedLength: number,
): IdentitySecretKey {
  return exactBytes(bytes, expectedLength, "IdentitySecretKey") as IdentitySecretKey;
}

export function bleIdentity(bytes: Uint8Array): BleIdentityValidationOutcome {
  return bytes.length === BLE_IDENTITY_LENGTH
    ? Tag("ValidBleIdentity", copyBytes(bytes) as BleIdentity)
    : Tag("InvalidBleIdentity", { actualLength: bytes.length });
}

export function channelTag(bytes: Uint8Array): ChannelTag {
  return nonEmptyBytes(bytes, "ChannelTag") as ChannelTag;
}

export function packetFrame(bytes: Uint8Array): PacketFrame {
  return nonEmptyBytes(bytes, "PacketFrame") as PacketFrame;
}

export function packetFrameView(bytes: Uint8Array): PacketFrame {
  requireNonEmptyBytes(bytes, "PacketFrame");
  return bytes as PacketFrame;
}

export function entropyBytes(bytes: Uint8Array): EntropyBytes {
  if (bytes.length < MIN_ENTROPY_BYTES) {
    throw new PrnsValidationError(
      "invalid-length",
      `EntropyBytes requires at least ${MIN_ENTROPY_BYTES} bytes`,
    );
  }
  return copyBytes(bytes) as EntropyBytes;
}

export function appData(bytes: Uint8Array = new Uint8Array()): AppData {
  return copyBytes(bytes) as AppData;
}

export function appName(value: string): AppName {
  return dottedComponent(value, "AppName") as AppName;
}

export function aspect(value: string): Aspect {
  return dottedComponent(value, "Aspect") as Aspect;
}

export function bitrateBps(value: number): BitrateBps {
  return positiveInteger(value, "BitrateBps") as BitrateBps;
}

export function hardwareMtu(value: number): HardwareMtu {
  return positiveInteger(value, "HardwareMtu") as HardwareMtu;
}

export function hopCount(value: number): HopCount {
  if (!Number.isInteger(value) || value < 0 || value > 255) {
    throw new PrnsValidationError(
      "invalid-number",
      "HopCount must be an integer from 0 through 255",
    );
  }
  return value as HopCount;
}

export function nowMillis(value: number = Date.now()): InstantMillis {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new PrnsValidationError(
      "invalid-number",
      "InstantMillis must be a non-negative safe integer",
    );
  }
  return value as InstantMillis;
}

export function commandId(value: bigint): CommandId {
  if (value < 0n) {
    throw new PrnsValidationError(
      "invalid-number",
      "CommandId must be non-negative",
    );
  }
  return value as CommandId;
}

export function copyBytes(bytes: Uint8Array): Uint8Array {
  return new Uint8Array(bytes);
}

export function positiveInteger(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new PrnsValidationError(
      "invalid-number",
      `${name} must be a positive safe integer`,
    );
  }
  return value;
}

export function nonNegativeInteger(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new PrnsValidationError(
      "invalid-number",
      `${name} must be a non-negative safe integer`,
    );
  }
  return value;
}

function exactBytes(
  bytes: Uint8Array,
  expectedLength: number,
  name: string,
): Uint8Array {
  if (bytes.length !== expectedLength) {
    throw new PrnsValidationError(
      "invalid-length",
      `${name} must be ${expectedLength} bytes`,
    );
  }
  return copyBytes(bytes);
}

function nonEmptyBytes(bytes: Uint8Array, name: string): Uint8Array {
  requireNonEmptyBytes(bytes, name);
  return copyBytes(bytes);
}

function requireNonEmptyBytes(bytes: Uint8Array, name: string): void {
  if (bytes.length === 0) {
    throw new PrnsValidationError("empty-bytes", `${name} must not be empty`);
  }
}

function dottedComponent(value: string, name: string): string {
  if (value.length === 0) {
    throw new PrnsValidationError("empty-string", `${name} must not be empty`);
  }
  if (value.includes(".")) {
    throw new PrnsValidationError(
      "invalid-component",
      `${name} must not contain dots`,
    );
  }
  return value;
}
